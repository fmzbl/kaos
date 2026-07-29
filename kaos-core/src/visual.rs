//! The mandala canvas as data, plus Rebis code generation and loading.
//!
//! [`rebis_lang::mandala`] projects Rebis source *into* the `o-[]-o` notation.
//! This module is the inverse: a drawable graph that generates Rebis source,
//! and loads it back. It is what `kaos visual` edits.
//!
//! # The abstraction
//!
//! Every Rebis expression is a **form** tag, a **text** payload, an optional
//! postfix **model binding**, and an **ordered list of children**.
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
//! (% condition when-one when-zero) Binary gate —    3
//! ($ a b …)           Concat    —               list
//! (a b …)             Compose   —               list
//! (f a b …)           Call      name            arguments
//! (~ f (p …) body)    Function  name + params   1 (the body)
//! a b …               Program   —               list (top level only)
//! expr/model           any form  model           the same children
//! ```
//!
//! So the whole language is one node type — [`Form`] plus text — and edges.
//! A node's children are numbered per parent from `1` through `n`, where `n`
//! is that parent's child count. [`Mandala::to_rebis`] folds those positions
//! into source and [`Mandala::from_rebis`] unfolds source back onto the canvas;
//! every form round-trips one-to-one. Invalid or incomplete graphs are
//! reported rather than repaired with invisible expressions.
//!
//! The whiteboard alphabet is a *rendering* of this, not a restriction on it:
//! prompts, symbols, and combining forms use the `o-[]-o` outlines; source
//! sigils—including `^`—are their own drawn shapes. Circle and square
//! boundaries render source indentation; only explicit flow operators render
//! arrows ([`Form::shape`]).
//!
//! This module is pure and std-only — no UI, no rendering, no I/O — so the
//! editor front-end is a thin shell over it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::OnceLock;

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
    /// `(% condition when-one when-zero)` — lazy binary control flow.
    Conditional,
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
    /// Flow forms remain available to source-oriented callers. The visual
    /// editor creates complete flow operators with its two-boundary gesture
    /// rather than exposing incomplete arrow nodes in the palette.
    pub const ALL: &'static [FormSpec] = &[
        ("⬡ prompt", || Form::Prompt, "prompt"),
        ("◇ symbol", || Form::Symbol, "x"),
        ("→ forward", || Form::Forward, ""),
        ("← backflow", || Form::Backflow, ""),
        ("[] square", || Form::Square, ""),
        ("% binary gate", || Form::Conditional, ""),
        ("( ) compose", || Form::Compose, ""),
        ("$ concat", || Form::Concat, ""),
        ("call", || Form::Call, "f"),
        ("~ macro", || Form::Function(vec!["x".into()]), "f"),
        ("# import", || Form::Import, "std/flow"),
        ("& input", || Form::Input, "input"),
        ("' quote", || Form::Quote, ""),
        (", unquote", || Form::Unquote, ""),
        ("^ invert", || Form::Invert, ""),
        ("△ program", || Form::Program, ""),
    ];

    /// Whether the form writes its operands inside delimiters of its own.
    ///
    /// This is the whole nesting rule, stated once. A form that opens its own
    /// `( )` or `[ ]` is an indentation: the forms it holds are written one
    /// level in from it, and so they are *drawn* one level in from it. Anything
    /// else — a prefix sigil like `'x`, a leaf, a program's newline-joined
    /// top level — writes its operand at its own level and opens nothing.
    ///
    /// Flow is the deliberate exception. `(-> a b)` does write its own
    /// parentheses, but it is drawn as the connection between two blocks rather
    /// than as a boundary around them, so it opens no indentation on the canvas.
    #[must_use]
    pub fn opens_indentation(&self) -> bool {
        matches!(
            self,
            Form::Compose
                | Form::Square
                | Form::Concat
                | Form::Invert
                | Form::Conditional
                | Form::Function(_)
                | Form::Input
                | Form::Call
                | Form::Forward
                | Form::Backflow
        )
    }

    /// Whether the form routes one answer into the next.
    ///
    /// Flow is the one indentation that also draws something *between* the two
    /// forms it holds: its circle is the expression, and the arrow inside is
    /// the direction. Everywhere the renderer asks "is this a flow" it must ask
    /// the form, not the outline — the outline is a circle like every other
    /// indentation's.
    #[must_use]
    pub fn is_flow(&self) -> bool {
        matches!(self, Form::Forward | Form::Backflow)
    }

    /// How the form is drawn.
    ///
    /// Every indentation is a boundary, because on this canvas the boundary IS
    /// the indentation: one `( )` in the source is one circle, `[ ]` is the one
    /// square, and the sigil that opened the parentheses is written on the ring
    /// of its own circle rather than left loose among the operands it governs.
    ///
    /// What remains outside that rule keeps the whiteboard alphabet: prefix
    /// sigils are drawn as the sigil itself (`'`, `,`, `#`) because they open
    /// nothing, prompts are text-bearing hexagons, programs are quiet triangles,
    /// and flow is the arrow between blocks.
    pub fn shape(&self) -> Shape {
        match self {
            Form::Prompt => Shape::Hexagon,
            Form::Symbol => Shape::Diamond,
            Form::Import => Shape::Hash,
            Form::Quote => Shape::Quote,
            Form::Unquote => Shape::Comma,
            Form::Square => Shape::Square,
            Form::Program => Shape::Triangle,
            // Every parenthesised form is one indentation, and every indentation
            // is one circle. The sigil it was written with becomes that circle's
            // mark; see `Node::mark`.
            Form::Compose
            | Form::Concat
            | Form::Function(_)
            | Form::Invert
            | Form::Call
            | Form::Input
            | Form::Conditional
            // A flow expression is parenthesised like any other, so it is an
            // indentation and gets the circle — which is what gives the arrow
            // above it a block to connect to. It wears no mark: a circle titled
            // with an arrow is nothing the palette can build, and the arrow it
            // would name is already drawn inside it.
            | Form::Forward
            | Form::Backflow => Shape::Circle,
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
            Form::Conditional => Arity::Exactly(3),
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
            Form::Conditional => "binary gate",
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
    /// `( )` — an ordered composition boundary. A compose node is the only
    /// circular form on the canvas.
    Circle,
    /// `△` — an implicit top-level program. Its name stays hidden until the
    /// node is selected, keeping the idle glyph purely structural.
    Triangle,
    /// `◇` — a symbol: a name rather than a literal.
    Diamond,
    /// `[]` — the mediator square, and nothing else. The box belongs to the
    /// one form whose notation is a box, so a square on the canvas always
    /// means a mediation.
    Square,
    /// A prompt terminal. The six sides leave a broad interior for its complete
    /// wrapped text while remaining distinct from every nesting boundary.
    Hexagon,
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
    /// `%` — a lazy binary gate.
    Percent,
    /// `->` / `<-` — drawn as the arrow between its two children. Incomplete
    /// flow forms use the same shape as a small selectable arrow node.
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
    /// The unscaled half-width and half-height of this symbol.
    ///
    /// These bounds are the common reference used by resizing, hit-testing,
    /// automatic layout, and renderers. A hand-set [`Node::size`] scales the
    /// symbol away from this size; keeping the reference beside the geometry
    /// prevents those consumers from inventing subtly different footprints.
    #[must_use]
    pub fn base_extent(self) -> (f64, f64) {
        match self {
            Shape::Circle
            | Shape::Diamond
            | Shape::Dollar
            | Shape::Tilde
            | Shape::Hash
            | Shape::Quote
            | Shape::Comma
            | Shape::Caret
            | Shape::Percent => (NODE_R, NODE_R),
            Shape::Parallelogram => (NODE_R + NODE_RY * 0.55, NODE_RY),
            Shape::Amp => (NODE_R + NODE_RY * 0.85, NODE_RY),
            Shape::Triangle | Shape::Square | Shape::Hexagon | Shape::Arrow => (NODE_R, NODE_RY),
        }
    }

    /// The strokes that draw this shape's sigil, or empty for the shapes that
    /// are outlines ([`Shape::Circle`], [`Shape::Triangle`],
    /// [`Shape::Square`], [`Shape::Diamond`]).
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
            // A percent sign: diagonal slash with two compact open circles.
            Shape::Percent => &[
                Stroke::Poly(&[
                    (-12.0, -12.0),
                    (-15.0, -10.0),
                    (-15.0, -6.0),
                    (-12.0, -4.0),
                    (-8.0, -6.0),
                    (-8.0, -10.0),
                    (-12.0, -12.0),
                ]),
                Stroke::Poly(&[
                    (12.0, 4.0),
                    (8.0, 6.0),
                    (8.0, 10.0),
                    (12.0, 12.0),
                    (15.0, 10.0),
                    (15.0, 6.0),
                    (12.0, 4.0),
                ]),
                Stroke::Poly(&[(-11.0, 15.0), (11.0, -15.0)]),
            ],
            Shape::Circle
            | Shape::Triangle
            | Shape::Square
            | Shape::Diamond
            | Shape::Arrow
            | Shape::Parallelogram
            | Shape::Hexagon
            | Shape::Amp => &[],
        }
    }

    /// The four corners of the diamond, in node-local coordinates.
    pub fn diamond_points() -> [(f32, f32); 4] {
        let r = NODE_R as f32;
        [(0.0, -r), (r, 0.0), (0.0, r), (-r, 0.0)]
    }

    /// The three corners of a prompt, pointing upward.
    pub fn triangle_points() -> [(f32, f32); 3] {
        let (r, ry) = (NODE_R as f32, NODE_RY as f32);
        [(0.0, -ry), (r, ry), (-r, ry)]
    }

    /// A compact right-facing arrow used while a flow form is incomplete.
    /// Complete flows are drawn as the connection between their operands.
    pub fn arrow_points() -> [(f32, f32); 7] {
        [
            (-10.0, -4.0),
            (2.0, -4.0),
            (2.0, -8.0),
            (10.0, 0.0),
            (2.0, 8.0),
            (2.0, 4.0),
            (-10.0, 4.0),
        ]
    }

    /// The four corners of the call parallelogram: a box sheared to the right.
    pub fn parallelogram_points() -> [(f32, f32); 4] {
        let (r, ry) = (NODE_R as f32, NODE_RY as f32);
        let s = ry * 0.55;
        [(-r + s, -ry), (r + s, -ry), (r - s, ry), (-r - s, ry)]
    }

    /// The six corners of a prompt hexagon: a box with both ends drawn to a
    /// point, leaving a broad text-bearing interior.
    pub fn hexagon_points() -> [(f32, f32); 6] {
        let (r, ry) = (NODE_R as f32, NODE_RY as f32);
        let cut = r * 0.42;
        [
            (-r, 0.0),
            (-r + cut, -ry),
            (r - cut, -ry),
            (r, 0.0),
            (r - cut, ry),
            (-r + cut, ry),
        ]
    }

    /// The five corners of the input inlet: a box with a leftward point where
    /// the received value enters.
    pub fn inlet_points() -> [(f32, f32); 5] {
        let (r, ry) = (NODE_R as f32, NODE_RY as f32);
        let tip = ry * 0.85;
        [(-r - tip, 0.0), (-r, -ry), (r, -ry), (r, ry), (-r, ry)]
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
            Shape::Square | Shape::Parallelogram | Shape::Amp | Shape::Hexagon => {
                dx.abs() <= NODE_R && dy.abs() <= NODE_RY
            }
            Shape::Triangle => {
                (-NODE_RY..=NODE_RY).contains(&dy)
                    && dx.abs() <= NODE_R * (dy + NODE_RY) / (2.0 * NODE_RY)
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
    /// Optional postfix model selector. `None` inherits the run's default;
    /// `Some` routes every model call in this node's subtree until a nested
    /// node overrides it.
    pub model: Option<String>,
    pub x: f64,
    pub y: f64,
    /// Half-width and half-height this symbol was dragged to, when it has been
    /// resized by hand. `None` means the symbol uses its natural size. A
    /// compose circle stores the same radius in both dimensions; a visual
    /// container may grow beyond the stored size to keep its contents visible.
    ///
    /// Presentation only, exactly like `x`/`y`: a hand-sized box and an
    /// auto-sized one generate the same source. Read it through
    /// [`Mandala::extent`], which never lets a stored size hide the contents.
    pub size: Option<(f64, f64)>,
    /// User-authored displacement in the structural 3D projection.
    ///
    /// Like 2D position and size this is presentation only: moving a piece
    /// along the 3D editor's X/Y/Z gizmo never changes the Rebis expression.
    /// The derived cone layout supplies the base position and this offset is
    /// added afterward, so structural depth remains available while pieces can
    /// be arranged by hand.
    pub spatial_offset: [f64; 3],
}

impl Node {
    pub fn shape(&self) -> Shape {
        self.form.shape()
    }

    /// This symbol's unscaled half-extents.
    #[must_use]
    pub fn base_extent(&self) -> (f64, f64) {
        self.shape().base_extent()
    }

    /// The sigil written on an indentation's ring.
    ///
    /// A circle is one `( )`, and this is the notation that opened it: `$` for
    /// concatenation, `~ f` for a macro, the callee's name for a call. It is
    /// drawn on the boundary rather than inside it, so it names the indentation
    /// without standing among the forms indented within.
    ///
    /// Empty for a bare `( )` compose, which was opened by nothing but the
    /// parenthesis, and for every form that is not an indentation.
    /// Only the head of the list — never its arguments.
    ///
    /// `(~ f (x) body)` wears `~`, not `~ f (x)`: the ring says which kind of
    /// indentation this is, and the panel says the rest. Writing the name and
    /// parameters up there turned the boundary's label back into the block it
    /// was supposed to have replaced. A call is the exception only because its
    /// head *is* the callee's name.
    #[must_use]
    pub fn mark(&self) -> String {
        match &self.form {
            Form::Concat => "$".into(),
            Form::Invert => "^".into(),
            Form::Conditional => "%".into(),
            Form::Input => "&".into(),
            Form::Function(_) => "~".into(),
            Form::Call => self.text.clone(),
            _ => String::new(),
        }
    }

    /// The single token the canvas draws for this form.
    ///
    /// A drawing is a map, not a document. One symbol says which form this is —
    /// the operator that opened it, or the first word of the text it carries —
    /// and the panel beside the canvas holds the whole of it, editable. Setting
    /// a form's complete payload inside its own outline is what made its text
    /// grow with the form until it covered whatever was nested underneath.
    ///
    /// Nothing is lost by this: it is a smaller view of text that is still
    /// there, one click away.
    #[must_use]
    pub fn glyph(&self) -> String {
        let mark = self.mark();
        if !mark.is_empty() {
            return mark;
        }
        let caption = self.caption();
        let mut token = caption.split_whitespace().next().unwrap_or_default();
        let elided = token.len() < caption.trim().len();
        let mut cut = token.chars().count() > GLYPH_TOKEN_CHARS;
        if cut {
            let end = token
                .char_indices()
                .nth(GLYPH_TOKEN_CHARS)
                .map_or(token.len(), |(index, _)| index);
            token = &token[..end];
        }
        cut |= elided;
        if cut {
            format!("{token}…")
        } else {
            token.to_string()
        }
    }

    /// Short canvas caption written *inside* the shape.
    ///
    /// Empty when the shape already says everything — a `,` needs no label
    /// written across it, and an indentation carries its notation on the ring
    /// (see [`Self::mark`]) so its interior stays clear for what it holds.
    pub fn caption(&self) -> String {
        if self.form.opens_indentation() {
            return String::new();
        }
        if self.form.uses_text() {
            return self.text.clone();
        }
        match self.form {
            Form::Forward => "->".into(),
            Form::Backflow => "<-".into(),
            Form::Program => "program".into(),
            // Drawn as their own sigil; nothing to write on top.
            Form::Quote | Form::Unquote => String::new(),
            _ => self.form.name().into(),
        }
    }
}

/// A grab on a symbol's scale outline: which boundary and which axes it changes.
///
/// Both flags are set on a corner. The axes are carried because a drag on a
/// side wall must change only that dimension — grabbing the left wall of a wide
/// box and pulling down should not also make it short.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BorderGrab {
    pub id: NodeId,
    /// On a left or right wall: the drag sets the width.
    pub wide: bool,
    /// On a top or bottom wall: the drag sets the height.
    pub tall: bool,
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

    #[must_use]
    pub fn width(self) -> f64 {
        self.max_x - self.min_x
    }

    #[must_use]
    pub fn height(self) -> f64 {
        self.max_y - self.min_y
    }

    #[must_use]
    pub fn centre(self) -> (f64, f64) {
        (
            (self.min_x + self.max_x) * 0.5,
            (self.min_y + self.max_y) * 0.5,
        )
    }

    /// Grow to include a symbol's complete drawn bounds.
    fn absorb_node(&mut self, node: &Node, extent: (f64, f64)) {
        self.min_x = self.min_x.min(node.x - extent.0);
        self.max_x = self.max_x.max(node.x + extent.0);
        self.min_y = self.min_y.min(node.y - extent.1);
        self.max_y = self.max_y.max(node.y + extent.1);
    }

    fn intersects_node(self, node: &Node, extent: (f64, f64)) -> bool {
        node.x + extent.0 >= self.min_x
            && node.x - extent.0 <= self.max_x
            && node.y + extent.1 >= self.min_y
            && node.y - extent.1 <= self.max_y
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
                write!(f, "the selected block crosses several outside parents")
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

/// Compose radius and the base half-width of fixed shapes, in world units.
pub const NODE_R: f64 = 34.0;
/// The half-height of flatter fixed shapes and a square's base half-height.
pub const NODE_RY: f64 = NODE_R * 0.72;
/// Clearance between inlined contents and their container boundary.
pub const MEDIATOR_PAD: f64 = 9.0;
/// Clear canvas each form indented inside a boundary keeps around itself.
///
/// Separate from [`MEDIATOR_PAD`], which is the margin against the wall: this
/// is what the contents claim from one another, and it is the one number that
/// decides how airy a drawn indentation reads. Half a symbol's width, so
/// neighbours are plainly apart without drifting into separate groups.
pub const CONTENT_GAP: f64 = 12.0;

/// How many characters of a form's text the canvas shows before the panel takes
/// over. Enough to tell two forms apart at a glance, not enough to become a
/// paragraph drawn over whatever the form contains.
pub const GLYPH_TOKEN_CHARS: usize = 12;
/// How far either side of a symbol's scale outline still counts as grabbing it,
/// measured in *screen* pixels.
///
/// A wall is a visual target, so its grab band is a constant thickness under the
/// pointer rather than a constant distance on the canvas. Callers divide by the
/// view zoom to get the world-space band [`Mandala::border_hit`] wants; without
/// that, zooming out shrinks the wall past reach and zooming in swells it until
/// the whole shape reads as its own border.
pub const BORDER_BAND: f64 = 8.0;
/// Grab radius of the handle on a drawn arrow.
pub const ARROW_HANDLE: f64 = 11.0;
/// Selection tolerance around a rendered flow line.
pub const ARROW_HIT_SLOP: f64 = 7.0;

/// Which part of the infinite canvas is on screen.
///
/// `tx`/`ty` are a screen-space translation and `zoom` a scale, so a renderer
/// maps this straight onto one transform. Pan is unbounded in every direction
/// and the wheel is unbounded in both: neither end of the zoom has an artificial
/// stop. The only limit left is arithmetic — see [`View::MIN_ZOOM`].
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
    /// The smallest scale the transform still survives, not a limit on how far
    /// the canvas may be pushed away.
    ///
    /// Scrolling out is as unbounded as scrolling in; what cannot be crossed is
    /// arithmetic. [`Self::to_world`] divides by the zoom, and renderers carry
    /// it in `f32`, so below the smallest positive `f32` the scale collapses to
    /// nothing and the view stops being invertible. That leaves some thirty-odd
    /// orders of magnitude of zoom-out — the wheel will not reach the end of it,
    /// and the drawing has been a sub-pixel speck for most of the way there.
    pub const MIN_ZOOM: f64 = f32::MIN_POSITIVE as f64;

    pub fn new() -> Self {
        Self::default()
    }

    /// The margin a fitted view leaves between the drawing and the window edge,
    /// in screen pixels, so a boundary's outline never sits flush against the
    /// frame.
    pub const FIT_MARGIN: f64 = 48.0;

    /// The view that brings `bounds` completely on screen, centred.
    ///
    /// This is the whole-drawing view: it scales down by however much the
    /// drawing needs to fit inside `width` × `height`, and never magnifies past
    /// natural size. A single small form is shown at 100% in the middle of the
    /// canvas rather than blown across it, which is what makes the answer
    /// "everything, readable" instead of merely "everything, technically".
    ///
    /// Degenerate input — an empty window, a drawing with no area, anything
    /// non-finite — falls back to the identity view rather than inventing a
    /// scale from a division by nothing.
    #[must_use]
    pub fn fitting(bounds: WorldRect, width: f64, height: f64) -> Self {
        let usable = (
            width - Self::FIT_MARGIN * 2.0,
            height - Self::FIT_MARGIN * 2.0,
        );
        if !usable.0.is_finite() || !usable.1.is_finite() || usable.0 <= 0.0 || usable.1 <= 0.0 {
            return Self::new();
        }
        let (drawn_w, drawn_h) = (bounds.width(), bounds.height());
        // A drawing with no extent in one axis is only constrained by the other.
        let by_width = (drawn_w > 0.0).then(|| usable.0 / drawn_w);
        let by_height = (drawn_h > 0.0).then(|| usable.1 / drawn_h);
        let zoom = match (by_width, by_height) {
            (Some(w), Some(h)) => w.min(h),
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => 1.0,
        }
        .clamp(Self::MIN_ZOOM, 1.0);
        let (cx, cy) = bounds.centre();
        let view = Self {
            tx: width * 0.5 - cx * zoom,
            ty: height * 0.5 - cy * zoom,
            zoom,
        };
        if view.tx.is_finite() && view.ty.is_finite() && view.zoom.is_finite() {
            view
        } else {
            Self::new()
        }
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
    /// so zooming follows the pointer instead of the origin. There is no
    /// arbitrary upper bound: growth stops only if the next transform cannot
    /// be represented by finite `f64` values.
    pub fn zoom_at(&mut self, sx: f64, sy: f64, factor: f64) {
        if !sx.is_finite() || !sy.is_finite() || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let (wx, wy) = self.to_world(sx, sy);
        let zoom = (self.zoom * factor).max(Self::MIN_ZOOM);
        let tx = sx - wx * zoom;
        let ty = sy - wy * zoom;
        if zoom.is_finite() && tx.is_finite() && ty.is_finite() {
            self.zoom = zoom;
            self.tx = tx;
            self.ty = ty;
        }
    }
}

/// A drawn mandala: forms plus the arrows between them.
#[derive(Debug)]
pub struct Mandala {
    nodes: Vec<Node>,
    arrows: Vec<Arrow>,
    /// Presentation-only membership for forms deliberately held inside a
    /// circle or square. These use the same child-to-container orientation as
    /// structural arrows, but never participate in source generation.
    contents: Vec<Arrow>,
    next_id: u32,
    /// Derived presentation geometry. It is deliberately absent from clones:
    /// document history stores the drawing, not another copy of its cache.
    geometry: OnceLock<MandalaGeometry>,
}

#[derive(Debug, Default)]
struct MandalaGeometry {
    fits: HashMap<NodeId, (f64, f64)>,
    extents: HashMap<NodeId, (f64, f64)>,
    inlined: HashSet<NodeId>,
}

impl Clone for Mandala {
    fn clone(&self) -> Self {
        Self {
            nodes: self.nodes.clone(),
            arrows: self.arrows.clone(),
            contents: self.contents.clone(),
            next_id: self.next_id,
            geometry: OnceLock::new(),
        }
    }
}

impl Default for Mandala {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            arrows: Vec::new(),
            contents: Vec::new(),
            next_id: 0,
            geometry: OnceLock::new(),
        }
    }
}

impl MandalaGeometry {
    fn from_mandala(mandala: &Mandala) -> Self {
        let containers = mandala
            .nodes
            .iter()
            .filter(|node| is_visual_container(&node.form))
            .map(|node| node.id)
            .collect::<Vec<_>>();
        let container_indices = containers
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index))
            .collect::<HashMap<_, _>>();

        // `interior` is explicit presentation membership and iterative. Compute
        // it exactly once per container: re-walking it from every extent query
        // was the source of exponential behaviour in deeply nested drawings.
        let mut inlined = HashSet::new();
        let interiors = containers
            .iter()
            .map(|id| {
                let mut interior = mandala.interior(*id);
                // `hold` rejects cycles, but a boundary is defensively never
                // counted among its own contents.
                interior.remove(id);
                inlined.extend(interior.iter().copied());
                interior.into_iter().collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        // A container depends on the extent of every nested container.
        // Collapse strongly connected components before sizing: a recursive
        // component has no finite padded fixed point, so internal references
        // use each member's hand/base size while acyclic dependencies retain
        // their exact computed extent.
        let dependencies = interiors
            .iter()
            .map(|interior| {
                interior
                    .iter()
                    .filter_map(|id| container_indices.get(id).copied())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let components = strongly_connected_components(&dependencies);
        let component_count = components
            .iter()
            .copied()
            .max()
            .map_or(0, |largest| largest + 1);
        let mut members = vec![Vec::new(); component_count];
        for (container, component) in components.iter().copied().enumerate() {
            members[component].push(container);
        }

        let mut component_dependencies = vec![Vec::new(); component_count];
        for (container, nested) in dependencies.iter().enumerate() {
            let component = components[container];
            for nested in nested {
                let nested_component = components[*nested];
                if nested_component != component {
                    component_dependencies[component].push(nested_component);
                }
            }
        }
        for dependencies in &mut component_dependencies {
            dependencies.sort_unstable();
            dependencies.dedup();
        }
        let mut dependents = vec![Vec::new(); component_count];
        for (component, dependencies) in component_dependencies.iter().enumerate() {
            for dependency in dependencies {
                dependents[*dependency].push(component);
            }
        }
        let mut pending = component_dependencies
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>();
        let mut ready = pending
            .iter()
            .enumerate()
            .filter_map(|(component, count)| (*count == 0).then_some(component))
            .collect::<VecDeque<_>>();

        let mut fits = mandala
            .nodes
            .iter()
            .map(|node| (node.id, default_extent(node)))
            .collect::<HashMap<_, _>>();
        let mut extents = mandala
            .nodes
            .iter()
            .map(|node| (node.id, base_extent(node)))
            .collect::<HashMap<_, _>>();

        while let Some(component) = ready.pop_front() {
            for container_index in &members[component] {
                let container_id = containers[*container_index];
                let Some(container) = mandala.node(container_id) else {
                    continue;
                };
                let mut fit = default_extent(container);
                for inner_id in &interiors[*container_index] {
                    let Some(inner) = mandala.node(*inner_id) else {
                        continue;
                    };
                    let child_extent = container_indices.get(inner_id).map_or_else(
                        || base_extent(inner),
                        |inner_index| {
                            if components[*inner_index] == component {
                                base_extent(inner)
                            } else {
                                extents
                                    .get(inner_id)
                                    .copied()
                                    .unwrap_or_else(|| base_extent(inner))
                            }
                        },
                    );
                    let far_x = (inner.x - container.x).abs() + child_extent.0;
                    let far_y = (inner.y - container.y).abs() + child_extent.1;
                    if is_circular_boundary(&container.form) {
                        // A circle must reach the farthest point of what it
                        // holds. For a boxy form that is the corner of its
                        // bounds — but a round one has no corners, and reaching
                        // for its phantom corner is what made every nesting
                        // level inflate its parent by a further √2. A circle
                        // inside a circle needs only the distance between their
                        // centres plus the inner radius.
                        let reach = if is_circular_boundary(&inner.form) {
                            (inner.x - container.x).hypot(inner.y - container.y) + child_extent.0
                        } else {
                            far_x.hypot(far_y)
                        };
                        let radius = reach + MEDIATOR_PAD;
                        fit = (fit.0.max(radius), fit.1.max(radius));
                    } else {
                        fit.0 = fit.0.max(far_x + MEDIATOR_PAD);
                        fit.1 = fit.1.max(far_y + MEDIATOR_PAD);
                    }
                }
                fits.insert(container_id, fit);
                let base = base_extent(container);
                extents.insert(container_id, (fit.0.max(base.0), fit.1.max(base.1)));
            }

            for dependent in &dependents[component] {
                pending[*dependent] -= 1;
                if pending[*dependent] == 0 {
                    ready.push_back(*dependent);
                }
            }
        }

        Self {
            fits,
            extents,
            inlined,
        }
    }
}

/// Whether the form draws a boundary that holds its operands.
///
/// One rule and no list to remember: an indentation is a boundary, because the
/// boundary is how indentation is drawn. See [`Form::opens_indentation`].
fn is_visual_container(form: &Form) -> bool {
    form.opens_indentation()
}

/// Whether the boundary is a true circle, and so must keep one shared radius
/// and clear its contents' far corner rather than their bounding box.
fn is_circular_boundary(form: &Form) -> bool {
    is_visual_container(form) && form.shape() == Shape::Circle
}

/// Whether the form sizes on one shared scale rather than two free axes.
///
/// Everything except the square. A square is a box, and its two walls are
/// genuinely independent — that is what makes a mediator a container you can
/// make wide and shallow. Every other outline is a *shape*: a hexagon stretched
/// along one axis alone is not a bigger hexagon, it is a different figure, and
/// the alphabet only has the one. So they grow and shrink whole.
pub fn scales_uniformly(form: &Form) -> bool {
    form.shape() != Shape::Square
}

fn default_extent(node: &Node) -> (f64, f64) {
    node.base_extent()
}

fn base_extent(node: &Node) -> (f64, f64) {
    let base = default_extent(node);
    node.size.map_or(base, |(half_w, half_h)| {
        if is_circular_boundary(&node.form) {
            let radius = half_w.max(half_h).max(NODE_R);
            (radius, radius)
        } else {
            (half_w.max(base.0), half_h.max(base.1))
        }
    })
}

/// Component labels for a directed graph, without recursive traversal.
///
/// Visual graphs are user-authored and can be arbitrarily deep, so even the
/// cycle detector used to protect sizing must not consume call-stack depth.
fn strongly_connected_components(edges: &[Vec<usize>]) -> Vec<usize> {
    let mut visited = vec![false; edges.len()];
    let mut finish = Vec::with_capacity(edges.len());
    for start in 0..edges.len() {
        if visited[start] {
            continue;
        }
        let mut stack = vec![(start, false)];
        while let Some((node, expanded)) = stack.pop() {
            if expanded {
                finish.push(node);
                continue;
            }
            if std::mem::replace(&mut visited[node], true) {
                continue;
            }
            stack.push((node, true));
            for next in edges[node].iter().rev() {
                if !visited[*next] {
                    stack.push((*next, false));
                }
            }
        }
    }

    let mut reverse = vec![Vec::new(); edges.len()];
    for (node, next) in edges.iter().enumerate() {
        for next in next {
            reverse[*next].push(node);
        }
    }

    let mut components = vec![usize::MAX; edges.len()];
    let mut component = 0;
    while let Some(start) = finish.pop() {
        if components[start] != usize::MAX {
            continue;
        }
        components[start] = component;
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            for next in &reverse[node] {
                if components[*next] == usize::MAX {
                    components[*next] = component;
                    stack.push(*next);
                }
            }
        }
        component += 1;
    }
    components
}

impl Mandala {
    pub fn new() -> Self {
        Self::default()
    }

    /// Place a form and return its handle.
    pub fn add(&mut self, form: Form, text: impl Into<String>, x: f64, y: f64) -> NodeId {
        self.invalidate_geometry();
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.push(Node {
            id,
            form,
            text: text.into(),
            model: None,
            x,
            y,
            size: None,
            spatial_offset: [0.0; 3],
        });
        id
    }

    /// Make `father` the semantic parent of `child`, assigning `child` the next
    /// one-based position among its ordered operands. Use
    /// [`Self::set_child_number`] to change that position later.
    ///
    /// This is the low-level ordered AST relation retained for source and API
    /// compatibility. The visual editor normally calls [`Self::reparent`] and
    /// [`Self::hold`] together when a form crosses an indentation boundary.
    /// Duplicates and self-links are ignored. Storage remains child-to-parent
    /// because that is the direction source generation walks.
    pub fn father_of(&mut self, father: NodeId, child: NodeId) -> bool {
        if father == child || !self.has(father) || !self.has(child) {
            return false;
        }
        let arrow = Arrow {
            from: child,
            to: father,
        };
        if self.arrows.contains(&arrow) {
            return false;
        }
        self.arrows.push(arrow);
        self.invalidate_geometry();
        true
    }

    /// The form this one is an ordered operand of, if any.
    #[must_use]
    pub fn father(&self, child: NodeId) -> Option<NodeId> {
        self.arrows
            .iter()
            .find(|arrow| arrow.from == child)
            .map(|arrow| arrow.to)
    }

    /// Arrange one boundary's direct contents on the tightest spiral that holds
    /// them, each at the smallest size it may be drawn.
    ///
    /// One indentation is one level, and a level is drawn at one size. Each
    /// form's *fit* is measured first — for a leaf its own outline, for a
    /// boundary whatever its contents occupy — and then the largest of those
    /// governs the whole level. Three macros written side by side are three
    /// circles of the same size, whatever each happens to contain; reading
    /// which is which is the caption's job, not the geometry's.
    ///
    /// Measuring the fit is also what drops any size set by hand and any slack
    /// a previous arrangement left, so the level is as small as it can honestly
    /// be while still being uniform.
    ///
    /// Measuring from the fit rather than from the current size is also what
    /// makes this idempotent: formatting twice leaves the drawing exactly where
    /// formatting once put it. A full relayout finishes inner boundaries first,
    /// so by the time one is measured here its own contents have settled.
    fn layout_contents(&mut self, container: NodeId) {
        let Some(centre) = self.node(container).map(|node| (node.x, node.y)) else {
            return;
        };
        let contents = self
            .contained_children(container)
            .into_iter()
            .map(|id| (id, self.fit_extent(id)))
            .collect::<Vec<_>>();
        for (id, fit) in &contents {
            // Each form to its own fit: the least it may honestly be drawn at.
            self.resize(*id, fit.0, fit.1);
        }
        // Pack with the sizes they will actually be drawn at.
        let items = contents
            .iter()
            .map(|(id, _)| (*id, self.extent(*id)))
            .collect::<Vec<_>>();
        for (id, x, y) in spiral_spots(&items) {
            self.move_group_to(id, centre.0 + x, centre.1 + y);
        }
    }

    /// Put a new indentation *around* an existing form.
    ///
    /// The form keeps everything it holds and everything it belongs to; one
    /// level of indentation is inserted above it, taking its exact place in its
    /// father's operand order and in whatever boundary was drawing it. This is
    /// the gesture for "wrap this in a compose": placing a boundary on a
    /// boundary, rather than dragging an empty one out and moving the other
    /// inside it by hand.
    pub fn wrap(&mut self, id: NodeId, form: Form) -> Option<NodeId> {
        if !form.opens_indentation() || !self.has(id) {
            return None;
        }
        let (x, y) = self.node(id).map(|node| (node.x, node.y))?;
        let father = self.father(id);
        let number = father.and_then(|father| self.child_number(father, id));
        let holder = self.holder(id);

        let made = self.add(form, String::new(), x, y);
        // `reparent` replaces the old father, so the order matters: the new
        // boundary claims the form first, then inherits the place the form used
        // to occupy.
        self.reparent(made, id);
        if let Some(father) = father {
            self.reparent(father, made);
            if let Some(number) = number {
                self.set_child_number(father, made, number);
            }
        }
        if let Some(holder) = holder {
            self.hold(holder, made);
        }
        self.hold(made, id);
        Some(made)
    }

    /// Put a new form *inside* a boundary, as its newest operand.
    ///
    /// The counterpart of [`Self::wrap`]: same gesture, other direction. Any
    /// form may be nested, not only another boundary — placing a prompt inside
    /// a circle is the ordinary way to fill one in, and clicking there used to
    /// do nothing but reselect the circle it landed on.
    ///
    /// The new form takes the next place on its boundary's spiral rather than
    /// landing on whatever already sits in the middle.
    pub fn nest(&mut self, id: NodeId, form: Form, text: impl Into<String>) -> Option<NodeId> {
        if !is_visual_container(&self.node(id)?.form) {
            return None;
        }
        let (x, y) = self.node(id).map(|node| (node.x, node.y))?;
        let made = self.add(form, text, x, y);
        self.reparent(id, made);
        self.hold(id, made);
        self.layout_contents(id);
        Some(made)
    }

    /// Make `child` one ordered operand of `parent`, replacing any previous
    /// structural parent.
    ///
    /// This is the semantic half of dropping a form into a circle or square:
    /// visual nesting becomes source nesting without exposing a second link
    /// gesture. A parent inside the child's own subtree would create
    /// a cycle and is refused atomically.
    pub fn reparent(&mut self, parent: NodeId, child: NodeId) -> bool {
        if parent == child
            || !self.has(parent)
            || !self.has(child)
            || self.subtree(child).contains(&parent)
        {
            return false;
        }
        let already_only_parent = {
            let parents = self
                .arrows
                .iter()
                .filter(|arrow| arrow.from == child)
                .collect::<Vec<_>>();
            parents.len() == 1 && parents[0].to == parent
        };
        if already_only_parent {
            return false;
        }
        self.arrows.retain(|arrow| arrow.from != child);
        self.arrows.push(Arrow {
            from: child,
            to: parent,
        });
        self.invalidate_geometry();
        true
    }

    /// Detach a form from its structural parent when it is pulled out of its
    /// indentation boundary.
    pub fn detach(&mut self, child: NodeId) -> bool {
        let before = self.arrows.len();
        self.arrows.retain(|arrow| arrow.from != child);
        if self.arrows.len() == before {
            return false;
        }
        self.invalidate_geometry();
        true
    }

    /// Hold `content` inside a circle or square as presentation state.
    ///
    /// The low-level content membership remains separate from the ordered AST
    /// relation so source loading and copy/paste can reconstruct both layers
    /// exactly. The editor's boundary-crossing gesture updates both. A form
    /// belongs to at most one direct container, and visual containment cycles
    /// are rejected.
    pub fn hold(&mut self, container: NodeId, content: NodeId) -> bool {
        if container == content
            || !self.has(content)
            || !self
                .node(container)
                .is_some_and(|node| is_visual_container(&node.form))
            || self.content_group(content).contains(&container)
        {
            return false;
        }
        let membership = Arrow {
            from: content,
            to: container,
        };
        if self.contents.contains(&membership) {
            return false;
        }
        self.contents.retain(|edge| edge.from != content);
        self.contents.push(membership);
        self.invalidate_geometry();
        true
    }

    /// Release a directly held form back onto the open canvas.
    pub fn release(&mut self, content: NodeId) -> bool {
        let before = self.contents.len();
        self.contents.retain(|edge| edge.from != content);
        if self.contents.len() == before {
            return false;
        }
        self.invalidate_geometry();
        true
    }

    /// The circle or square directly holding `content`, if any.
    #[must_use]
    pub fn holder(&self, content: NodeId) -> Option<NodeId> {
        self.contents
            .iter()
            .find(|edge| edge.from == content)
            .map(|edge| edge.to)
    }

    /// The topmost circle or square whose current boundary contains the point.
    ///
    /// `content` is excluded along with anything it already contains so a
    /// container can be dropped inside another one without ever containing
    /// itself.
    #[must_use]
    pub fn holder_at(&self, content: NodeId, x: f64, y: f64) -> Option<NodeId> {
        let excluded = self.content_group(content);
        self.nodes.iter().rev().find_map(|node| {
            (is_visual_container(&node.form)
                && !excluded.contains(&node.id)
                && self.node_contains(node, x, y))
            .then_some(node.id)
        })
    }

    /// Push unrelated forms out of a container after its content makes the
    /// boundary grow.
    ///
    /// An AST child without content membership is unrelated to visual
    /// containment and is therefore pushed outside like any other loose form.
    /// Only explicit content remains within the boundary. Nested containers
    /// move with their contents.
    pub fn make_room_for_container(&mut self, id: NodeId) {
        let Some(container) = self.node(id).cloned() else {
            return;
        };
        if !is_visual_container(&container.form) {
            return;
        }
        let container_extent = self.extent(id);
        let interior = self.interior(id);

        // Never displace a structural ancestor that contains this boundary.
        let mut ancestors = HashSet::new();
        let mut stack = vec![id];
        while let Some(child) = stack.pop() {
            let structural = self
                .arrows
                .iter()
                .filter(|arrow| arrow.from == child)
                .map(|arrow| arrow.to);
            let holders = self
                .contents
                .iter()
                .filter(|edge| edge.from == child)
                .map(|edge| edge.to);
            for father in structural.chain(holders) {
                if ancestors.insert(father) {
                    stack.push(father);
                }
            }
        }

        let inlined_pass = self.is_inlined(id);
        let mut candidates = self
            .nodes
            .iter()
            .filter(|node| {
                node.id != id
                    && !interior.contains(&node.id)
                    && !ancestors.contains(&node.id)
                    && self.is_inlined(node.id) == inlined_pass
            })
            .map(|node| {
                let extent = self.extent(node.id);
                (node.clone(), extent)
            })
            .filter(|(node, extent)| {
                (node.x - container.x).abs() < container_extent.0 + extent.0
                    && (node.y - container.y).abs() < container_extent.1 + extent.1
            })
            .collect::<Vec<_>>();
        // Move a nested container before any of its contents, then mark that
        // whole group as handled so its arrangement is not torn apart.
        candidates.sort_by_key(|(node, _)| !is_visual_container(&node.form));

        let mut moved = HashSet::new();
        for (node, extent) in candidates {
            if moved.contains(&node.id) {
                continue;
            }
            let destinations = [
                (
                    container.x - container_extent.0 - extent.0 - MEDIATOR_PAD,
                    node.y,
                ),
                (
                    container.x + container_extent.0 + extent.0 + MEDIATOR_PAD,
                    node.y,
                ),
                (
                    node.x,
                    container.y - container_extent.1 - extent.1 - MEDIATOR_PAD,
                ),
                (
                    node.x,
                    container.y + container_extent.1 + extent.1 + MEDIATOR_PAD,
                ),
            ];
            let destination = destinations
                .into_iter()
                .min_by(|left, right| {
                    let left_distance = (left.0 - node.x).abs() + (left.1 - node.y).abs();
                    let right_distance = (right.0 - node.x).abs() + (right.1 - node.y).abs();
                    left_distance.total_cmp(&right_distance)
                })
                .unwrap_or((node.x, node.y));
            if is_visual_container(&node.form) {
                moved.extend(self.interior(node.id));
                self.move_group_to(node.id, destination.0, destination.1);
            } else {
                self.move_to(node.id, destination.0, destination.1);
            }
            moved.insert(node.id);
        }
    }

    /// Backward-compatible name for callers that only know about square
    /// containers.
    pub fn make_room_for_square(&mut self, id: NodeId) {
        self.make_room_for_container(id);
    }

    /// Compatibility name using the internal child-to-parent order.
    ///
    /// This remains a low-level compatibility alias for adding an ordered AST
    /// child. Visual nesting should use [`Self::reparent`] plus [`Self::hold`].
    /// A flow arrow has the distinct semantics in [`Self::flow`].
    pub fn connect(&mut self, from: NodeId, to: NodeId) {
        let _ = self.father_of(to, from);
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
        let _ = self.father_of(id, from);
        let _ = self.father_of(id, to);
        Some(id)
    }

    pub fn disconnect(&mut self, from: NodeId, to: NodeId) {
        let before = self.arrows.len();
        self.arrows.retain(|a| a.from != from || a.to != to);
        if self.arrows.len() != before {
            self.invalidate_geometry();
        }
    }

    /// Remove a node and every arrow touching it.
    pub fn remove(&mut self, id: NodeId) {
        let nodes_before = self.nodes.len();
        let arrows_before = self.arrows.len();
        let contents_before = self.contents.len();
        self.nodes.retain(|n| n.id != id);
        self.arrows.retain(|a| a.from != id && a.to != id);
        self.contents
            .retain(|edge| edge.from != id && edge.to != id);
        if self.nodes.len() != nodes_before
            || self.arrows.len() != arrows_before
            || self.contents.len() != contents_before
        {
            self.invalidate_geometry();
        }
    }

    pub fn set_text(&mut self, id: NodeId, text: impl Into<String>) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.text = text.into();
        }
    }

    /// Set or clear one node's postfix model selector.
    pub fn set_model(&mut self, id: NodeId, model: Option<String>) {
        if let Some(node) = self.nodes.iter_mut().find(|node| node.id == id) {
            node.model = model.filter(|value| !value.is_empty());
        }
    }

    pub fn set_form(&mut self, id: NodeId, form: Form) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            if n.form != form {
                n.form = form;
                if !is_visual_container(&n.form) {
                    self.contents.retain(|edge| edge.to != id);
                }
                self.invalidate_geometry();
            }
        }
    }

    pub fn move_to(&mut self, id: NodeId, x: f64, y: f64) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            if n.x != x || n.y != y {
                n.x = x;
                n.y = y;
                self.invalidate_geometry();
            }
        }
    }

    /// Set one node's presentation-only displacement in the 3D editor.
    pub fn set_spatial_offset(&mut self, id: NodeId, offset: [f64; 3]) -> bool {
        let Some(node) = self.nodes.iter_mut().find(|node| node.id == id) else {
            return false;
        };
        if node.spatial_offset == offset {
            return false;
        }
        node.spatial_offset = offset;
        true
    }

    /// Reset every hand-moved 3D piece to the derived structural layout.
    pub fn reset_spatial_offsets(&mut self) -> bool {
        let mut changed = false;
        for node in &mut self.nodes {
            if node.spatial_offset != [0.0; 3] {
                node.spatial_offset = [0.0; 3];
                changed = true;
            }
        }
        changed
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn arrows(&self) -> &[Arrow] {
        &self.arrows
    }

    /// The result endpoint of a complete flow expression.
    ///
    /// A nested `->` is still one Rebis operand, but it has no independent
    /// visual box. Its parent therefore connects to the nested flow's result
    /// endpoint instead of to an invisible midpoint handle. Incomplete arrows
    /// deliberately return `None`, so the editor can draw their real node.
    #[must_use]
    pub fn flow_result(&self, id: NodeId) -> Option<NodeId> {
        let node = self.node(id)?;
        if !matches!(node.form, Form::Forward | Form::Backflow) {
            return None;
        }
        let children = self.children(id);
        let [first, second] = children[..] else {
            return None;
        };
        Some(if node.form == Form::Backflow {
            first
        } else {
            second
        })
    }

    /// Nodes touched by a world-space marquee.
    ///
    /// Every form uses its visual bounds, flow included: a flow is the circle
    /// holding the two forms it routes between, so the line drawn inside it is
    /// already covered by that boundary's own extent.
    #[must_use]
    pub fn nodes_in_rect(&self, rect: WorldRect) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|node| {
                let extent = self.extent(node.id);
                if !node.form.is_flow() {
                    return rect.intersects_node(node, extent);
                }
                let children = self.children(node.id);
                let [first, second] = children[..] else {
                    return rect.intersects_node(node, extent);
                };
                let Some(first) = self.node(first) else {
                    return rect.intersects_node(node, extent);
                };
                let Some(second) = self.node(second) else {
                    return rect.intersects_node(node, extent);
                };
                rect.intersects_node(node, extent)
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
            contents: self
                .contents
                .iter()
                .filter(|edge| ids.contains(&edge.from) && ids.contains(&edge.to))
                .copied()
                .collect(),
            next_id: self.next_id,
            geometry: OnceLock::new(),
        }
    }

    /// Append an exact copy of `source`, assigning fresh ids and translating
    /// every node by `offset`.
    ///
    /// Node and link order are retained, so ordered operands mean the same
    /// thing after a block is pasted. Links to anything outside `source`
    /// cannot leak into the copy.
    pub fn append_copy(&mut self, source: &Self, offset: (f64, f64)) -> Vec<NodeId> {
        self.invalidate_geometry();
        let mut remap = HashMap::new();
        let mut pasted = Vec::with_capacity(source.nodes.len());
        for node in &source.nodes {
            let id = NodeId(self.next_id);
            self.next_id += 1;
            let mut copy = node.clone();
            copy.id = id;
            copy.x += offset.0;
            copy.y += offset.1;
            self.nodes.push(copy);
            remap.insert(node.id, id);
            pasted.push(id);
        }
        for arrow in &source.arrows {
            let (Some(&child), Some(&father)) = (remap.get(&arrow.from), remap.get(&arrow.to))
            else {
                continue;
            };
            let _ = self.father_of(father, child);
        }
        for edge in &source.contents {
            let (Some(&content), Some(&container)) = (remap.get(&edge.from), remap.get(&edge.to))
            else {
                continue;
            };
            let _ = self.hold(container, content);
        }
        pasted
    }

    /// Top-level expressions inside a selection, in stable canvas order.
    ///
    /// Nested selected operands are already represented through their selected
    /// parent, so folding uses only these roots as the new form's children.
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

    /// Fold the selected subgraph into one new parent form.
    ///
    /// Existing internal structure is preserved. If the selection occupied
    /// one slot (or several sibling slots) under an outside parent, that
    /// boundary is rewired through the new form at the first selected slot.
    /// Crossing several outside parents is rejected instead of creating a
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
            let _ = self.father_of(folded, root);
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
        // Contents drawn inside a container are on top of it: clicking an
        // inner form must select the form, not its surrounding boundary.
        let inside =
            self.nodes.iter().rev().find_map(|n| {
                (self.is_inlined(n.id) && self.node_contains(n, x, y)).then_some(n.id)
            });
        if inside.is_some() {
            return inside;
        }
        let shape = self
            .nodes
            .iter()
            .rev()
            .find_map(|n| self.node_contains(n, x, y).then_some(n.id));
        if shape.is_some() {
            return shape;
        }
        // A flow expression is rendered as the complete blue line between its
        // children. Its midpoint handle is useful but must not be the only
        // selectable part of the form.
        self.nodes.iter().rev().find_map(|node| {
            if !node.form.is_flow() {
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

    fn node_contains(&self, node: &Node, x: f64, y: f64) -> bool {
        let (dx, dy) = (x - node.x, y - node.y);
        let (half_w, half_h) = self.extent(node.id);
        let (base_w, base_h) = node.base_extent();
        let local_x = dx / (half_w / base_w).max(f64::EPSILON);
        let local_y = dy / (half_h / base_h).max(f64::EPSILON);
        node.shape().contains(local_x, local_y)
    }

    /// Children of `id`: the nodes whose arrows point at it, in draw order.
    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        self.arrows
            .iter()
            .filter(|a| a.to == id)
            .map(|a| a.from)
            .collect()
    }

    /// `id` and everything nested inside it through explicit content
    /// membership. Ordered AST children are intentionally not walked.
    fn content_group(&self, id: NodeId) -> std::collections::BTreeSet<NodeId> {
        let mut group = std::collections::BTreeSet::new();
        let mut stack = vec![id];
        while let Some(container) = stack.pop() {
            if !group.insert(container) {
                continue;
            }
            stack.extend(
                self.contents
                    .iter()
                    .filter(|edge| edge.to == container)
                    .map(|edge| edge.from),
            );
        }
        group
    }

    /// The mediator of a square: its first child.
    ///
    /// `([m] a b)` writes the mediator INSIDE the brackets, so it is part of
    /// the square's own notation rather than one of its arguments — even
    /// though the AST holds it as a child expression, which is what lets a
    /// whole program mediate.
    #[must_use]
    pub fn mediator(&self, id: NodeId) -> Option<NodeId> {
        let node = self.node(id)?;
        if node.form != Form::Square {
            return None;
        }
        self.children(id).first().copied()
    }

    /// The mediator drawn inside its square, when it has explicitly been made
    /// that square's content.
    ///
    /// Source loading marks the mediator and branches as square content. A
    /// mediator added later through the low-level AST API remains outside until
    /// it is deliberately placed into the box.
    #[must_use]
    pub fn inlined_mediator(&self, id: NodeId) -> Option<NodeId> {
        self.mediator(id)
            .filter(|mediator| self.contains_child(id, *mediator))
    }

    /// Forms explicitly held directly within a visual boundary.
    ///
    /// This is presentation state, separate from [`Self::children`]. A form may
    /// be both source content and an ordered AST child, but the low-level AST
    /// relation alone never puts it here.
    #[must_use]
    pub fn contained_children(&self, id: NodeId) -> Vec<NodeId> {
        let Some(node) = self.node(id) else {
            return Vec::new();
        };
        if !is_visual_container(&node.form) {
            return Vec::new();
        }
        // In operand order, so a boundary's spiral reads the way its source
        // does: the first child nearest the middle, the last furthest out.
        // Returning them in the order they happened to be *held* scattered the
        // arrangement, and the numbers the inspector shows for child order then
        // matched nothing on the canvas.
        let mut held = self
            .contents
            .iter()
            .filter(|edge| edge.to == id)
            .map(|edge| edge.from)
            .collect::<Vec<_>>();
        let ordered = self.children(id);
        held.sort_by_key(|content| {
            // Anything held without being an ordered operand keeps its place
            // after the operands, in the order it was put there.
            ordered
                .iter()
                .position(|child| child == content)
                .unwrap_or(usize::MAX)
        });
        held
    }

    /// Whether `child` is directly held as `container`'s visual content.
    #[must_use]
    pub fn contains_child(&self, container: NodeId, child: NodeId) -> bool {
        self.contents.contains(&Arrow {
            from: child,
            to: container,
        })
    }

    /// The nodes of one paint pass, back to front.
    ///
    /// `inlined` picks the pass: `false` is everything on the open canvas,
    /// `true` is what is drawn inside a visual boundary. Within a pass the
    /// containers come first, because their fills are surfaces other forms sit
    /// on.
    ///
    /// Order only. Which node is in front of which is presentation, exactly like
    /// the coordinates it is derived from; no structure is implied and none is
    /// read back out.
    #[must_use]
    pub fn paint_order(&self, inlined: bool) -> Vec<NodeId> {
        let pass = self
            .nodes
            .iter()
            .filter(|node| self.is_inlined(node.id) == inlined);
        let (containers, forms): (Vec<NodeId>, Vec<NodeId>) = pass
            .map(|node| (node.id, is_visual_container(&node.form)))
            .fold((Vec::new(), Vec::new()), |mut acc, (id, container)| {
                if container {
                    acc.0.push(id);
                } else {
                    acc.1.push(id);
                }
                acc
            });
        containers.into_iter().chain(forms).collect()
    }

    /// Every node drawn inside `id`'s boundary.
    ///
    /// This follows explicit content membership recursively, never bare AST
    /// edges. Merely overlapping a boundary still means nothing until the drop
    /// gesture assigns that membership.
    #[must_use]
    pub fn interior(&self, id: NodeId) -> std::collections::BTreeSet<NodeId> {
        let mut interior = self
            .contained_children(id)
            .into_iter()
            .flat_map(|content| self.content_group(content))
            .collect::<std::collections::BTreeSet<_>>();
        // Defensive even though `hold` rejects containment cycles.
        interior.remove(&id);
        interior
    }

    /// Whether this node is drawn inside some explicit visual container.
    #[must_use]
    pub fn is_inlined(&self, id: NodeId) -> bool {
        self.geometry().inlined.contains(&id)
    }

    /// The node's half-width and half-height in world units.
    ///
    /// Every form uses its base outline or its hand-set size. Squares and
    /// compose circles may grow farther around explicitly held content. A
    /// hand-set container boundary is honoured only where it is larger than
    /// the fit, so contents cannot end up outside.
    #[must_use]
    pub fn extent(&self, id: NodeId) -> (f64, f64) {
        self.geometry()
            .extents
            .get(&id)
            .copied()
            .unwrap_or((NODE_R, NODE_RY))
    }

    /// The smallest the node may be drawn: its natural symbol size, or for a
    /// visual container whatever its contents occupy. This is the floor
    /// [`Self::extent`] and [`Self::resize`] measure a hand-set size against.
    #[must_use]
    fn fit_extent(&self, id: NodeId) -> (f64, f64) {
        self.geometry()
            .fits
            .get(&id)
            .copied()
            .unwrap_or((NODE_R, NODE_RY))
    }

    /// Set a symbol's drawn size by hand, in half-extents from its centre.
    ///
    /// Clamped to the symbol's natural footprint and, for a visual container,
    /// what its contents need.
    ///
    /// Only a square changes its two axes independently — see
    /// [`scales_uniformly`]. Every other form keeps the proportions of its own
    /// outline: one factor is taken from whichever axis was asked for more, and
    /// both half-extents follow it, so a circle stays round and a hexagon stays
    /// a hexagon at any size.
    ///
    /// The centre does not move and the contents are left exactly where they
    /// are, at the size they already were. Resizing a form is about that form:
    /// a boundary's wall travels, and what stands inside it is untouched. The
    /// wall still stops at its contents, so it can be widened freely and only
    /// closed as far as they allow.
    pub fn resize(&mut self, id: NodeId, half_w: f64, half_h: f64) {
        let (fit_w, fit_h) = self.fit_extent(id);
        if let Some(node) = self.nodes.iter_mut().find(|node| node.id == id) {
            node.size = Some(if scales_uniformly(&node.form) {
                let (base_w, base_h) = node.base_extent();
                let axis = |half: f64, base: f64| {
                    if base > 0.0 && half.is_finite() {
                        half / base
                    } else {
                        1.0
                    }
                };
                // The requested scale, never below the form's own outline nor
                // below what its contents already occupy.
                let scale = axis(half_w, base_w)
                    .max(axis(half_h, base_h))
                    .max(axis(fit_w, base_w))
                    .max(axis(fit_h, base_h))
                    .max(1.0);
                (base_w * scale, base_h * scale)
            } else {
                (half_w.max(fit_w), half_h.max(fit_h))
            });
            self.invalidate_geometry();
        }
    }

    /// Size an indentation and scale everything drawn inside it to match.
    ///
    /// A boundary and its contents read as one object — dragging it anywhere
    /// but its wall already carries the interior along, via
    /// [`Self::move_group_to`]. Growing it is that same object under a different
    /// gesture: the wall travels and the forms indented inside travel and grow
    /// with it, so the drawing keeps its arrangement instead of becoming a
    /// larger and larger empty room around symbols that stayed a speck.
    ///
    /// Positions and extents take the *same* factor. They have to: scaling the
    /// sizes while pinning the positions would march neighbours into each other,
    /// so "scale the contents" can only mean the whole interior at once.
    ///
    /// The factor is one number — see [`Self::group_scale`] — so nothing inside
    /// is distorted, and a pair that cleared each other before still clears
    /// afterwards.
    pub fn resize_group(&mut self, id: NodeId, half_w: f64, half_h: f64) {
        let factor = self.group_scale(id, half_w, half_h);
        if let Some(centre) = self
            .node(id)
            .map(|node| (node.x, node.y))
            .filter(|_| (factor - 1.0).abs() > f64::EPSILON)
        {
            // Every scaled position and size is read before any is written: the
            // derived geometry is invalidated by each edit, so measuring as we
            // went would compound the factor across the interior.
            let scaled = self
                .interior(id)
                .into_iter()
                .filter_map(|inner| {
                    let node = self.node(inner)?;
                    let extent = self.extent(inner);
                    Some((
                        inner,
                        centre.0 + (node.x - centre.0) * factor,
                        centre.1 + (node.y - centre.1) * factor,
                        (extent.0 * factor, extent.1 * factor),
                    ))
                })
                .collect::<Vec<_>>();
            for (inner, x, y, size) in scaled {
                self.move_to(inner, x, y);
                self.set_size(inner, Some(size));
            }
        }
        self.resize(id, half_w, half_h);
    }

    /// How much [`Self::resize_group`] scales an indentation's contents.
    ///
    /// One uniform number, taken from the tighter requested axis and held above
    /// the point where the first symbol inside would be drawn smaller than the
    /// size its own shape is naturally drawn at. From there the wall keeps
    /// closing on its own and comes to rest on its contents.
    fn group_scale(&self, id: NodeId, half_w: f64, half_h: f64) -> f64 {
        let Some(node) = self.node(id).filter(|node| is_visual_container(&node.form)) else {
            return 1.0;
        };
        let (want_w, want_h) = if scales_uniformly(&node.form) {
            let (base_w, base_h) = node.base_extent();
            let scale = (half_w / base_w.max(f64::EPSILON)).max(half_h / base_h.max(f64::EPSILON));
            (base_w * scale, base_h * scale)
        } else {
            (half_w, half_h)
        };
        let before = self.extent(id);
        let factor = (want_w / before.0).min(want_h / before.1);
        if !factor.is_finite() || factor <= 0.0 {
            return 1.0;
        }
        factor.max(self.smallest_group_scale(id))
    }

    /// The most an indentation's contents may shrink: the point at which the
    /// first of them reaches the size its own shape is naturally drawn at.
    fn smallest_group_scale(&self, id: NodeId) -> f64 {
        self.interior(id)
            .into_iter()
            .filter_map(|inner| {
                let node = self.node(inner)?;
                let extent = self.extent(inner);
                let base = node.base_extent();
                Some((base.0 / extent.0).max(base.1 / extent.1))
            })
            .filter(|floor| floor.is_finite())
            .fold(0.0_f64, f64::max)
    }

    /// The complete drawn bounds of everything on the canvas, extents included.
    ///
    /// `None` for an empty drawing, which has no bounds to speak of rather than
    /// a zero-sized one at the origin.
    #[must_use]
    pub fn bounds(&self) -> Option<WorldRect> {
        let mut bounds: Option<WorldRect> = None;
        for node in &self.nodes {
            let extent = self.extent(node.id);
            match &mut bounds {
                Some(bounds) => bounds.absorb_node(node, extent),
                none => {
                    *none = Some(WorldRect::from_points(
                        (node.x - extent.0, node.y - extent.1),
                        (node.x + extent.0, node.y + extent.1),
                    ))
                }
            }
        }
        bounds
    }

    /// Set — or clear — a symbol's hand-set size outright.
    ///
    /// [`Self::resize`] is the gesture: it clamps to what the symbol and its
    /// contents need. This is the bookkeeping underneath it, and the only way
    /// back to an automatic size, so a boundary held at a fixed size for the
    /// length of a drag can be handed back to its contents afterwards.
    pub fn set_size(&mut self, id: NodeId, size: Option<(f64, f64)>) {
        if let Some(node) = self.nodes.iter_mut().find(|node| node.id == id) {
            if node.size != size {
                node.size = size;
                self.invalidate_geometry();
            }
        }
    }

    /// A symbol's hand-set size, if it has one.
    #[must_use]
    pub fn hand_size(&self, id: NodeId) -> Option<(f64, f64)> {
        self.node(id).and_then(|node| node.size)
    }

    /// The wall a pointer at `x`/`y` may drag to size a box, if any.
    ///
    /// A wall the pointer is on, minus any wall with something else drawn over
    /// it: a block dropped on a border is still a block, and grabbing it must
    /// move it rather than reshape the box underneath. That is the whole rule
    /// for "the border resizes, the items do not", kept here so the canvas and
    /// its cursor cannot read it two different ways.
    ///
    /// `band` is the world-space grab tolerance — see [`BORDER_BAND`].
    #[must_use]
    pub fn resize_grab(&self, x: f64, y: f64, band: f64) -> Option<BorderGrab> {
        let over = self.hit(x, y);
        // The wall of whatever the pointer is actually resting on comes first.
        // Without this, a form sitting near its container's wall made the
        // container ungrabbable: the form's own band reached the same point, so
        // the wall found there belonged to the content while the point itself
        // was inside the container — a mismatch that cancelled the gesture and
        // left the boundary unable to be resized at all.
        if let Some(id) = over {
            if let Some(grab) = self.node_border(id, x, y, band) {
                return Some(grab);
            }
        }
        let grab = self.border_hit(x, y, band)?;
        match over {
            // Something else is drawn over the wall, so that is what the gesture
            // takes. Nothing there — which is the case just OUTSIDE the wall —
            // leaves the wall itself, and must not fall through to a canvas pan.
            Some(id) if id != grab.id => None,
            _ => Some(grab),
        }
    }

    /// Whether this one form's scale outline passes under the point.
    fn node_border(&self, id: NodeId, x: f64, y: f64, band: f64) -> Option<BorderGrab> {
        if !band.is_finite() || band <= 0.0 {
            return None;
        }
        let node = self.node(id)?;
        let (half_w, half_h) = self.extent(id);
        // Half the shape at most: past that the outer band still reaches out as
        // far as asked, but the inner edge stops short of the centre.
        let band = band.min(half_w.min(half_h) * 0.5);
        if is_circular_boundary(&node.form) {
            let distance = (x - node.x).hypot(y - node.y);
            return ((distance - half_w).abs() <= band).then_some(BorderGrab {
                id,
                wide: true,
                tall: true,
            });
        }
        let (dx, dy) = ((x - node.x).abs(), (y - node.y).abs());
        if dx > half_w + band || dy > half_h + band {
            return None;
        }
        let on_wide = dx >= half_w - band;
        let on_tall = dy >= half_h - band;
        if !on_wide && !on_tall {
            return None;
        }
        // Only a square reports the single wall that was grabbed. Every other
        // outline scales whole, so any part of its border governs both axes at
        // once and the cursor says so.
        let uniform = scales_uniformly(&node.form);
        Some(BorderGrab {
            id,
            wide: uniform || on_wide,
            tall: uniform || on_tall,
        })
    }

    /// The symbol whose scale outline the point rests on, and which axes resize.
    ///
    /// The band straddles the boundary by `band` world units — the caller's
    /// screen-space [`BORDER_BAND`] divided by the current zoom, so the wall is
    /// equally reachable at every magnification. It is clamped below the shape's
    /// own half-extents so a deeply zoomed-out symbol can never become a target
    /// that is entirely border and has no interior left to grab.
    ///
    /// Rectangular scale outlines report the side/corner axes; a compose circle
    /// reports both axes because its radius must remain equal in every
    /// direction. Complete flow forms are represented by their connection and
    /// therefore have no separate scale outline.
    ///
    /// This is geometry only — see [`Self::resize_grab`] for the answer a pointer
    /// gesture wants.
    #[must_use]
    pub fn border_hit(&self, x: f64, y: f64, band: f64) -> Option<BorderGrab> {
        self.nodes
            .iter()
            .rev()
            .find_map(|node| self.node_border(node.id, x, y, band))
    }

    /// Move a visual container and everything explicitly held inside it.
    ///
    /// A boundary and its contents read as one object, so dragging it by the
    /// border or empty space carries the interior along. Plain
    /// [`Mandala::move_to`] still moves exactly one node, which is what layout
    /// wants.
    pub fn move_group_to(&mut self, id: NodeId, x: f64, y: f64) {
        let Some(node) = self.node(id) else {
            return;
        };
        let (dx, dy) = (x - node.x, y - node.y);
        let interior: Vec<NodeId> = self.interior(id).into_iter().collect();
        self.move_to(id, x, y);
        for inner in interior {
            if let Some(child) = self.node(inner) {
                let (cx, cy) = (child.x + dx, child.y + dy);
                self.move_to(inner, cx, cy);
            }
        }
    }

    /// The one-based position of `child` under `father`.
    #[must_use]
    pub fn child_number(&self, father: NodeId, child: NodeId) -> Option<usize> {
        self.children(father)
            .iter()
            .position(|id| *id == child)
            .map(|index| index + 1)
    }

    /// Assign `child` a one-based position under `father`.
    ///
    /// Positions are always in `1..=n`, where `n` is the number of children
    /// currently attached to `father`. Reordering changes only sibling order;
    /// it does not move nodes, rewrite payloads, or alter any other parent.
    /// Returns `false` when the relationship or position is invalid, or when
    /// the child already occupies that position.
    pub fn set_child_number(&mut self, father: NodeId, child: NodeId, number: usize) -> bool {
        let sibling_indices = self
            .arrows
            .iter()
            .enumerate()
            .filter(|(_, arrow)| arrow.to == father)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if number == 0 || number > sibling_indices.len() {
            return false;
        }
        let Some(current) = sibling_indices
            .iter()
            .position(|index| self.arrows[*index].from == child)
        else {
            return false;
        };
        let target = number - 1;
        if current == target {
            return false;
        }

        let mut ordered = sibling_indices
            .iter()
            .map(|index| self.arrows[*index].from)
            .collect::<Vec<_>>();
        let moved = ordered.remove(current);
        ordered.insert(target, moved);
        for (index, child) in sibling_indices.into_iter().zip(ordered) {
            self.arrows[index].from = child;
        }
        self.invalidate_geometry();
        true
    }

    fn geometry(&self) -> &MandalaGeometry {
        self.geometry
            .get_or_init(|| MandalaGeometry::from_mandala(self))
    }

    fn invalidate_geometry(&mut self) {
        self.geometry.take();
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
            let extent = mandala.extent(id);
            let own_weight = (extent.0.max(extent.1) / NODE_R).max(1.0);
            let total = if kids.is_empty() {
                own_weight
            } else {
                kids.iter()
                    .map(|kid| subtree_leaves(mandala, *kid, depth + 1, depths, memo))
                    .sum::<f64>()
                    .max(own_weight)
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
            let parent_radius = mandala.extent(id).0.max(mandala.extent(id).1);
            let child_radius = kids
                .iter()
                .map(|kid| {
                    let extent = mandala.extent(*kid);
                    extent.0.max(extent.1)
                })
                .fold(0.0_f64, f64::max);
            // Wide subtrees earn a wider ring; each layer draws its cone a
            // golden step tighter, with a floor so rings never collapse onto
            // the node itself.
            let ring = ((CONE_ARC_PER_LEAF * total / std::f64::consts::TAU).max(CONE_MIN_RING)
                * CONE_SHRINK.powf(depth as f64 * 0.5))
            .max(parent_radius + child_radius + MEDIATOR_PAD * 2.0)
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
                    x: x + if recursive { phase.cos() * 28.0 } else { 0.0 }
                        + node.spatial_offset[0],
                    y: y + if recursive { phase.sin() * 28.0 } else { 0.0 }
                        + node.spatial_offset[1],
                    z: depth as f64 * LAYER_GAP
                        + if recursive {
                            phase / std::f64::consts::TAU * 90.0
                        } else {
                            0.0
                        }
                        + node.spatial_offset[2],
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
        let (expression, model) = match expression {
            Expr::Model { selector, body } => (body.as_ref(), Some(selector.as_str())),
            expression => (expression, None),
        };
        if node.model.as_deref() != model {
            return false;
        }
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
            (
                Form::Conditional,
                Expr::Conditional {
                    condition,
                    when_yes,
                    when_no,
                },
            ) => {
                children.len() == 3
                    && child_matches(0, condition)
                    && child_matches(1, when_yes)
                    && child_matches(2, when_no)
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
            Form::Conditional => format!("(% {} {} {})", parts[0], parts[1], parts[2]),
            Form::Concat => format!("($ {})", parts.join(" ")),
            Form::Compose => format!("({})", parts.join(" ")),
            Form::Call => format!("({text} {})", parts.join(" ")).replace(" )", ")"),
            Form::Function(params) => {
                format!("(~ {text} ({}) {})", params.join(" "), parts[0])
            }
            Form::Input => format!("(& {text} {})", parts[0]),
            Form::Program => parts.join("\n"),
        };
        Ok(match &node.model {
            Some(model) => format!("{out}/{model}"),
            None => out,
        })
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
/// Clear canvas left between the resized bounds of vertically packed forms.
const ROW_GUTTER: f64 = ROW_GAP - NODE_R * 2.0;
/// The golden ratio. The column pitch is the row pitch times PHI, so the grid
/// the circuit sits on keeps a golden aspect — the earlier "divine geometry"
/// proportion, now expressed as the board's cell shape.
const PHI: f64 = 1.618_033_988_749_895;
/// Horizontal gap between columns (one nesting level), a golden step wider than
/// the row pitch so signals have room to route between stages.
const COL_GAP: f64 = ROW_GAP * PHI;
/// Clear canvas left between the resized bounds of adjacent depth columns.
const COLUMN_GUTTER: f64 = COL_GAP - NODE_R * 2.0;

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

/// Angle each successive item advances along the interior spiral. A shade under
/// a sixth of a turn, so one arm carries roughly six forms before it comes back
/// around and the next winding settles outside the one beneath it.
const SPIRAL_STEP: f64 = 1.0;

/// Keeps equality on the non-overlapping side despite floating-point rounding
/// in the geometry cache that measures these positions afterwards.
const SPIRAL_SLACK: f64 = 1.0 + 1e-9;

/// The open interval of radii along a ray for which this axis fails to separate.
///
/// A spot at radius `r` on a ray of direction `dir` has coordinate `r * dir` on
/// this axis, so it clears a neighbour sitting at `centre` exactly when
/// `|r * dir - centre| >= half`. `None` means the axis separates at every
/// radius; an infinite interval means it separates at none.
fn blocked_radii(dir: f64, centre: f64, half: f64) -> Option<(f64, f64)> {
    if dir.abs() <= f64::EPSILON {
        // The ray never moves along this axis, so the answer is the same
        // everywhere on it.
        return (centre.abs() < half).then_some((f64::NEG_INFINITY, f64::INFINITY));
    }
    let (near, far) = ((centre - half) / dir, (centre + half) / dir);
    Some((near.min(far), near.max(far)))
}

/// The smallest radius at or beyond zero that lies in none of the intervals.
fn first_clear_radius(mut blocked: Vec<(f64, f64)>) -> f64 {
    blocked.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut radius = 0.0_f64;
    for (low, high) in blocked {
        if low <= radius && radius < high {
            radius = high * SPIRAL_SLACK;
        }
    }
    radius
}

/// The tightest non-overlapping spiral positions for one boundary's direct
/// contents.
///
/// Each item takes the next ray around — a constant angular step, so the set
/// reads outward along one continuous arm — and is then pulled in along that ray
/// as far as it will go: to the smallest radius that clears every item already
/// placed, which is nearly always the middle for the first of them. A boundary
/// is only ever as large as its contents force it to be, so packing them at the
/// least radius each admits is what keeps the circle or square around them
/// shrunk to its contents.
///
/// The clearance test is the separating axis for two axis-aligned bounds: two
/// items miss each other when their X bounds separate or their Y bounds do.
/// Along one ray each axis fails over a single interval of radii, so the radii a
/// neighbour forbids are known exactly rather than searched for.
fn spiral_spots(items: &[(NodeId, (f64, f64))]) -> Vec<(NodeId, f64, f64)> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut spots: Vec<(NodeId, f64, f64, (f64, f64))> = Vec::with_capacity(items.len());
    for (index, (id, extent)) in items.iter().enumerate() {
        let angle = index as f64 * SPIRAL_STEP;
        let (cos, sin) = (angle.cos(), angle.sin());
        let blocked = spots
            .iter()
            .filter_map(|(_, x, y, placed)| {
                let half_w = extent.0 + placed.0 + CONTENT_GAP;
                let half_h = extent.1 + placed.1 + CONTENT_GAP;
                let (low_x, high_x) = blocked_radii(cos, *x, half_w)?;
                let (low_y, high_y) = blocked_radii(sin, *y, half_h)?;
                // Overlapping means failing to separate on BOTH axes at once.
                let (low, high) = (low_x.max(low_y), high_x.min(high_y));
                (low < high).then_some((low, high))
            })
            .collect::<Vec<_>>();
        let radius = first_clear_radius(blocked);
        spots.push((*id, radius * cos, radius * sin, *extent));
    }
    let (min_x, max_x, min_y, max_y) = spots.iter().fold(
        (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ),
        |(min_x, max_x, min_y, max_y), (_, x, y, extent)| {
            (
                min_x.min(x - extent.0),
                max_x.max(x + extent.0),
                min_y.min(y - extent.1),
                max_y.max(y + extent.1),
            )
        },
    );
    let centre = ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5);
    spots
        .drain(..)
        .map(|(id, x, y, _)| (id, x - centre.0, y - centre.1))
        .collect()
}

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
        let (expr, model) = match expr {
            Expr::Model { selector, body } => (body.as_ref(), Some(selector.to_string())),
            expr => (expr, None),
        };
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
            Expr::Conditional {
                condition,
                when_yes,
                when_no,
            } => (
                Form::Conditional,
                String::new(),
                vec![condition.as_ref(), when_yes.as_ref(), when_no.as_ref()],
            ),
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
            Expr::Model { .. } => unreachable!("model wrapper was unwrapped above"),
        };

        let indents = form.opens_indentation();
        let id = self.add(form, text, 0.0, 0.0);
        self.set_model(id, model);
        depths.push((id, depth));
        let mut child_ids = Vec::with_capacity(kids.len());
        for kid in kids {
            let child = self.build(kid, depth + 1, depths);
            let _ = self.father_of(id, child);
            child_ids.push(child);
        }

        // Source delimiters carry a literal visual interior. Auto-drawing a
        // compose keeps every form written at that indentation inside its
        // circle. A square likewise keeps its mediator AND every branch inside
        // the box: brackets/parentheses are the nesting, so their operands
        // never need an extra visible parent arrow. Nested visual containers
        // retain their nearer content; `hold_lexical_subtree` only claims
        // nodes that do not already have a holder.
        //
        // This applies while loading source. A low-level AST relation still
        // leaves its child outside until presentation membership is assigned;
        // the visual editor's boundary-crossing gesture assigns both.
        if indents {
            for child in child_ids {
                self.hold_lexical_subtree(id, child);
            }
        }
        id
    }

    /// Assign every unclaimed form in a source-written expression to its
    /// nearest visual delimiter.
    fn hold_lexical_subtree(&mut self, container: NodeId, root: NodeId) {
        let mut stack = vec![root];
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            stack.extend(self.children(id).into_iter().rev());
            if self.holder(id).is_none() {
                let _ = self.hold(container, id);
            }
        }
    }

    /// Lay the syntax tree out as a left-to-right, size-aware circuit.
    ///
    /// Nesting depth is the **column**. Column centres are separated by the
    /// largest symbol on either side, so resizing a circle or square also
    /// increases the visible indentation it occupies. Subtrees are packed into
    /// non-overlapping vertical bands using each symbol's current extent.
    ///
    /// Explicit contents are formatted first. Each circle or square receives a
    /// compact phyllotaxis layout: item `n` sits at `sqrt(n)` turns of the
    /// golden angle, with the smallest global radius coefficient that keeps all
    /// resized bounds apart. Nested containers are packed from the inside out.
    /// Coordinates and content placement are presentation only and never affect
    /// generated Rebis.
    fn layout(&mut self, depths: &[(NodeId, usize)]) {
        use std::cmp::Reverse;

        #[derive(Default)]
        struct PackedBand {
            top: f64,
            bottom: f64,
            ids: Vec<NodeId>,
        }

        #[allow(clippy::too_many_arguments)]
        fn pack_rows(
            mandala: &Mandala,
            id: NodeId,
            active: &HashSet<NodeId>,
            depth_of: &HashMap<NodeId, usize>,
            rows: &mut HashMap<NodeId, f64>,
            seen: &mut HashSet<NodeId>,
            cursor: &mut f64,
        ) -> Option<PackedBand> {
            if !active.contains(&id) || !seen.insert(id) {
                return None;
            }
            let start = *cursor;
            let depth = depth_of.get(&id).copied().unwrap_or_default();
            let mut child_bands = mandala
                .children(id)
                .into_iter()
                .filter(|child| active.contains(child))
                .filter(|child| depth_of.get(child).copied() == Some(depth + 1))
                .filter_map(|child| pack_rows(mandala, child, active, depth_of, rows, seen, cursor))
                .collect::<Vec<_>>();
            let half_h = mandala.extent(id).1;
            if child_bands.is_empty() {
                let centre = start + half_h;
                rows.insert(id, centre);
                *cursor = centre + half_h + ROW_GUTTER;
                return Some(PackedBand {
                    top: start,
                    bottom: centre + half_h,
                    ids: vec![id],
                });
            }

            let mut ids = child_bands
                .iter_mut()
                .flat_map(|band| std::mem::take(&mut band.ids))
                .collect::<Vec<_>>();
            let mut top = child_bands.first().map(|band| band.top).unwrap_or(start);
            let mut bottom = child_bands.last().map(|band| band.bottom).unwrap_or(start);
            let mut centre = (top + bottom) * 0.5;

            // A tall parent may protrude above its first child's band. Shift
            // the complete subtree down rather than allowing it to overlap the
            // previous root's band.
            let lift = (start - (centre - half_h)).max(0.0);
            if lift > 0.0 {
                for child in &ids {
                    if let Some(row) = rows.get_mut(child) {
                        *row += lift;
                    }
                }
                top += lift;
                bottom += lift;
                centre += lift;
                *cursor += lift;
            }

            rows.insert(id, centre);
            ids.push(id);
            top = top.min(centre - half_h);
            bottom = bottom.max(centre + half_h);
            *cursor = (*cursor).max(bottom + ROW_GUTTER);
            Some(PackedBand { top, bottom, ids })
        }

        let depth_of = depths.iter().copied().collect::<HashMap<NodeId, usize>>();

        // Pack nested boundaries first so an outer spiral sees the final extent
        // of every inner square or circle.
        let mut containers = self
            .nodes
            .iter()
            .filter(|node| {
                is_visual_container(&node.form) && !self.contained_children(node.id).is_empty()
            })
            .map(|node| node.id)
            .collect::<Vec<_>>();
        containers.sort_by_key(|container| {
            let mut depth = 0usize;
            let mut cursor = *container;
            let mut seen = HashSet::new();
            while seen.insert(cursor) {
                let Some(holder) = self.holder(cursor) else {
                    break;
                };
                depth += 1;
                cursor = holder;
            }
            Reverse(depth)
        });
        for container in containers {
            self.layout_contents(container);
        }

        // Interior forms consume space within their nearest boundary, not a
        // second row or column on the outer circuit.
        let active = depths
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| !self.is_inlined(*id))
            .collect::<HashSet<_>>();
        let mut rows = HashMap::new();
        let mut seen = HashSet::new();
        let mut cursor = 0.0;
        for (id, depth) in depths {
            if *depth == 0 {
                let _ = pack_rows(
                    self,
                    *id,
                    &active,
                    &depth_of,
                    &mut rows,
                    &mut seen,
                    &mut cursor,
                );
            }
        }
        // Shared or recursive drawings may not expose an ordinary depth-zero
        // path. Keep their remaining forms finite and deterministically packed.
        for (id, _) in depths {
            let _ = pack_rows(
                self,
                *id,
                &active,
                &depth_of,
                &mut rows,
                &mut seen,
                &mut cursor,
            );
        }

        let max_depth = active
            .iter()
            .filter_map(|id| depth_of.get(id))
            .copied()
            .max()
            .unwrap_or_default();
        let mut column_half_widths = vec![NODE_R; max_depth + 1];
        for id in &active {
            let depth = depth_of.get(id).copied().unwrap_or_default();
            column_half_widths[depth] = column_half_widths[depth].max(self.extent(*id).0);
        }
        let mut columns = vec![CIRCUIT_ORIGIN.0; max_depth + 1];
        for depth in 1..columns.len() {
            columns[depth] = columns[depth - 1]
                + column_half_widths[depth - 1]
                + COLUMN_GUTTER
                + column_half_widths[depth];
        }
        let first_row = rows.values().copied().fold(f64::INFINITY, f64::min);
        let first_row = if first_row.is_finite() {
            first_row
        } else {
            0.0
        };
        let positions = depths
            .iter()
            .filter(|(id, _)| active.contains(id))
            .map(|(id, depth)| {
                (
                    *id,
                    columns.get(*depth).copied().unwrap_or(CIRCUIT_ORIGIN.0),
                    CIRCUIT_ORIGIN.1 + rows.get(id).copied().unwrap_or(first_row) - first_row,
                )
            })
            .collect::<Vec<_>>();
        for (id, x, y) in positions {
            if self
                .node(id)
                .is_some_and(|node| is_visual_container(&node.form))
            {
                self.move_group_to(id, x, y);
            } else {
                self.move_to(id, x, y);
            }
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
        assert_round_trip("(% \"question\" \"yes\" \"no\")"); // Conditional
        assert_round_trip("($ \"x\" \"y\")"); // Concat
        assert_round_trip("((\"local\") \"sub\")"); // Compose
        assert_round_trip("(f \"a\" \"b\")"); // Call
        assert_round_trip("\"p\" \"q\""); // Program
        assert_round_trip("(-> \"a\" \"b\")/claude:opus5"); // Model
    }

    #[test]
    fn model_bindings_are_node_metadata_and_round_trip() {
        let source = "(-> \"a\"/ollama:qwen4:4b \
                      ([\"judge\"] \"b\" \"c\")/openrouter:anthropic/claude-opus-4)\
                      /claude:opus5";
        let mandala = Mandala::from_rebis(source).unwrap();

        // Model wrappers annotate existing forms; they do not add geometry.
        assert_eq!(mandala.nodes().len(), 6);
        let flow = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Forward)
            .expect("flow");
        let square = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Square)
            .expect("square");
        let prompt = mandala
            .nodes()
            .iter()
            .find(|node| node.text == "a")
            .expect("bound prompt");
        assert_eq!(flow.model.as_deref(), Some("claude:opus5"));
        assert_eq!(
            square.model.as_deref(),
            Some("openrouter:anthropic/claude-opus-4")
        );
        assert_eq!(prompt.model.as_deref(), Some("ollama:qwen4:4b"));
        assert!(mandala
            .nodes()
            .iter()
            .filter(|node| matches!(node.text.as_str(), "judge" | "b" | "c"))
            .all(|node| node.model.is_none()));

        let regenerated = mandala.to_rebis().unwrap();
        assert_eq!(
            rebis_lang::parse(&regenerated).unwrap(),
            rebis_lang::parse(source).unwrap()
        );
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
               (% (stop value) value (loop (work value) work stop)))\n\
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
        // `(^ x)` writes its own parentheses, so it is an indentation and is
        // drawn as the circle it opens, marked with the caret it was written
        // with.
        assert_eq!(inverter.shape(), Shape::Circle);
        assert_eq!(inverter.mark(), "^");
        assert_eq!(mandala.children(inverter.id).len(), 1);
    }

    #[test]
    fn percent_conditionals_round_trip_as_three_ordered_children() {
        let source = "(% (-> question decision) yes-branch no-branch)";
        let mandala = Mandala::from_rebis(source).unwrap();
        assert_eq!(mandala.to_rebis().unwrap(), source);
        let conditional = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Conditional)
            .expect("conditional node");
        assert_eq!(conditional.shape(), Shape::Circle);
        assert_eq!(conditional.mark(), "%");
        assert_eq!(mandala.children(conditional.id).len(), 3);
    }

    // ── shapes are a rendering of the form ─────────────────────────────────

    #[test]
    fn only_prefix_sigils_are_drawn_as_their_sigil() {
        // A sigil that opens no parentheses governs the form written straight
        // after it, at its own level, so it stays a mark on the canvas.
        assert_eq!(Form::Quote.shape(), Shape::Quote);
        assert_eq!(Form::Unquote.shape(), Shape::Comma);
        assert_eq!(Form::Import.shape(), Shape::Hash);
        for form in [Form::Quote, Form::Unquote, Form::Import] {
            assert!(!form.opens_indentation(), "{form:?} opens no indentation");
        }
    }

    #[test]
    fn the_canvas_draws_one_token_and_leaves_the_rest_to_the_panel() {
        let mut m = Mandala::new();

        // A long prompt shows its first word, marked as having more behind it.
        let long = m.add(
            Form::Prompt,
            "Investigate the retry queue and report every failure mode",
            0.0,
            0.0,
        );
        assert_eq!(m.node(long).unwrap().glyph(), "Investigate…");
        // The text itself is untouched — the panel still has all of it.
        assert_eq!(
            m.node(long).unwrap().text,
            "Investigate the retry queue and report every failure mode"
        );

        // A single short word is shown whole, with nothing to suggest more.
        let short = m.add(Form::Symbol, "decision", 0.0, 0.0);
        assert_eq!(m.node(short).unwrap().glyph(), "decision");

        // One long word is cut rather than allowed to run across the canvas.
        let ident = m.add(Form::Symbol, "surviving-verified-design", 0.0, 0.0);
        assert_eq!(m.node(ident).unwrap().glyph(), "surviving-ve…");

        // An indentation shows the sigil that opened it, never its text.
        let macro_form = m.add(Form::Function(vec!["x".into()]), "twice", 0.0, 0.0);
        assert_eq!(m.node(macro_form).unwrap().glyph(), "~");

        // And a bare compose has nothing to say: its circle is the statement.
        let compose = m.add(Form::Compose, "", 0.0, 0.0);
        assert_eq!(m.node(compose).unwrap().glyph(), "");
    }

    #[test]
    fn every_parenthesised_form_is_a_circle_wearing_its_own_sigil() {
        // One rule: a form that writes its operands inside its own parentheses
        // is an indentation, an indentation is a circle, and the notation that
        // opened it is written on the ring instead of loose among its contents.
        for (form, mark) in [
            (Form::Concat, "$"),
            (Form::Invert, "^"),
            (Form::Conditional, "%"),
            (Form::Function(vec![]), "~"),
            (Form::Input, "&"),
            (Form::Compose, ""),
        ] {
            assert!(form.opens_indentation(), "{form:?} is an indentation");
            assert_eq!(form.shape(), Shape::Circle, "{form:?} is drawn as one");
            let mut mandala = Mandala::new();
            let id = mandala.add(form.clone(), "", 0.0, 0.0);
            let node = mandala.node(id).expect("the form was just added");
            assert_eq!(node.mark(), mark, "{form:?} wears its own sigil");
            // Its interior belongs to what it holds, never to a label.
            assert_eq!(node.caption(), "", "{form:?} keeps its interior clear");
        }

        // The square is the other delimiter and keeps its own outline.
        assert_eq!(Form::Square.shape(), Shape::Square);
        assert!(Form::Square.opens_indentation());
        assert!(!Form::Square.is_flow());
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
    fn father_of_keeps_circle_and_square_children_outside() {
        for form in [Form::Square, Form::Compose] {
            let mut mandala = Mandala::new();
            let container = mandala.add(form.clone(), "", 0.0, 0.0);
            let child = mandala.add(Form::Prompt, "child", 0.0, 0.0);

            assert!(mandala.father_of(container, child));
            assert_eq!(mandala.children(container), vec![child]);
            assert!(
                mandala.contained_children(container).is_empty(),
                "a father link is not content for {form:?}"
            );
            assert!(!mandala.is_inlined(child));
            assert_eq!(
                mandala.extent(container),
                default_extent(mandala.node(container).unwrap())
            );

            mandala.make_room_for_container(container);
            let boundary = mandala.extent(container);
            let child_extent = mandala.extent(child);
            let child = mandala.node(child).unwrap();
            assert!(
                child.x.abs() > boundary.0 + child_extent.0
                    || child.y.abs() > boundary.1 + child_extent.1,
                "the child must be visibly outside {form:?}"
            );
        }
    }

    #[test]
    fn holding_content_is_visual_and_does_not_invent_a_child() {
        let mut mandala = Mandala::new();
        let circle = mandala.add(Form::Compose, "", 0.0, 0.0);
        let content = mandala.add(Form::Prompt, "content", 120.0, 0.0);

        assert!(mandala.hold(circle, content));
        assert_eq!(mandala.holder(content), Some(circle));
        assert_eq!(mandala.contained_children(circle), vec![content]);
        assert!(mandala.children(circle).is_empty());
        assert!(mandala.is_inlined(content));
        assert!(matches!(
            mandala.to_rebis(),
            Err(MandalaError::ManyRoots(_))
        ));
    }

    #[test]
    fn indentation_drop_can_reparent_and_detach_source_structure() {
        let mut mandala = Mandala::new();
        let circle = mandala.add(Form::Compose, "", 0.0, 0.0);
        let first = mandala.add(Form::Prompt, "first", 0.0, 0.0);
        let second = mandala.add(Form::Prompt, "second", 0.0, 0.0);

        assert!(mandala.reparent(circle, first));
        assert!(mandala.reparent(circle, second));
        assert!(mandala.hold(circle, first));
        assert!(mandala.hold(circle, second));
        assert_eq!(mandala.children(circle), vec![first, second]);
        assert_eq!(mandala.to_rebis().unwrap(), "(\"first\" \"second\")");

        assert!(mandala.detach(first));
        assert_eq!(mandala.children(circle), vec![second]);
        assert!(matches!(
            mandala.to_rebis(),
            Err(MandalaError::ManyRoots(_))
        ));
    }

    #[test]
    fn reparent_refuses_to_put_an_expression_inside_its_own_descendant() {
        let mut mandala = Mandala::new();
        let outer = mandala.add(Form::Compose, "", 0.0, 0.0);
        let inner = mandala.add(Form::Compose, "", 0.0, 0.0);
        assert!(mandala.reparent(outer, inner));
        let before = mandala.arrows.clone();

        assert!(!mandala.reparent(inner, outer));
        assert_eq!(mandala.arrows, before);
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
    fn hand_moving_a_3d_piece_changes_only_its_spatial_projection() {
        let mut mandala = Mandala::from_rebis("(\"a\" \"b\")").unwrap();
        let source = mandala.to_rebis().unwrap();
        let id = mandala
            .nodes()
            .iter()
            .find(|node| node.text == "a")
            .unwrap()
            .id;
        let before = mandala.spatial_layout().node(id).copied().unwrap();

        assert!(mandala.set_spatial_offset(id, [45.0, -30.0, 80.0]));
        let moved = mandala.spatial_layout().node(id).copied().unwrap();
        assert_eq!(
            (moved.x, moved.y, moved.z),
            (before.x + 45.0, before.y - 30.0, before.z + 80.0)
        );
        assert_eq!(mandala.to_rebis().unwrap(), source);

        assert!(mandala.reset_spatial_offsets());
        let restored = mandala.spatial_layout().node(id).copied().unwrap();
        assert_eq!(
            (restored.x, restored.y, restored.z),
            (before.x, before.y, before.z)
        );
        assert!(!mandala.reset_spatial_offsets());
    }

    #[test]
    fn spatial_layout_occupies_volume_rather_than_extruding_the_drawing() {
        // Two sibling subtrees under one square. In an extruded 2D layout the
        // siblings would share the drawing's plane; as a cone tree they fan
        // around their parent, so they differ on every axis.
        let mandala = Mandala::from_rebis("([\"m\"] (-> \"a\" \"b\") (-> \"c\" \"d\"))").unwrap();
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
        source.hold(father, first);

        let mut target = Mandala::new();
        let existing = target.add(Form::Symbol, "existing", 0.0, 0.0);
        let pasted = target.append_copy(&source, (24.0, 32.0));

        assert_eq!(pasted.len(), 3);
        assert!(pasted.iter().all(|id| *id != existing));
        let copied_father = pasted[0];
        assert_eq!(target.children(copied_father), vec![pasted[1], pasted[2]]);
        assert_eq!(target.holder(pasted[1]), Some(copied_father));
        assert_eq!(target.holder(pasted[2]), None);
        let copied = target.node(copied_father).unwrap();
        assert_eq!((copied.x, copied.y), (34.0, 52.0));
        assert_eq!(copied.form, Form::Compose);
        assert_eq!(target.node(pasted[1]).unwrap().text, "a");
        assert_eq!(target.node(pasted[2]).unwrap().text, "b");
    }

    #[test]
    fn appending_a_copy_preserves_square_and_circle_sizes() {
        let mut source = Mandala::new();
        let circle = source.add(Form::Compose, "", 10.0, 20.0);
        let square = source.add(Form::Square, "", 210.0, 220.0);
        source.resize(circle, 180.0, 120.0);
        source.resize(square, 210.0, 130.0);

        let copied = source.induced_subgraph([circle, square]);
        let mut target = Mandala::new();
        let pasted = target.append_copy(&copied, (24.0, 32.0));

        assert_eq!(target.node(pasted[0]).unwrap().size, Some((180.0, 180.0)));
        assert_eq!(target.extent(pasted[0]), (180.0, 180.0));
        assert_eq!(target.node(pasted[1]).unwrap().size, Some((210.0, 130.0)));
        assert_eq!(target.extent(pasted[1]), (210.0, 130.0));
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
    fn a_flow_is_an_untitled_circle_with_its_arrow_drawn_inside() {
        // `(-> A B)` is parenthesised like any other form, so it gets a circle —
        // which is what gives the arrow above it a block to point at, instead of
        // the nesting being flattened into one long chain. The circle wears no
        // mark: a circle titled with an arrow is nothing the palette can build,
        // and the arrow it would name is already drawn inside it.
        for flow in [Form::Forward, Form::Backflow] {
            assert!(flow.is_flow());
            assert!(flow.opens_indentation(), "a flow expression is a `( )`");
            assert_eq!(flow.shape(), Shape::Circle);
        }

        let m = Mandala::from_rebis("(-> (-> \"a\" \"b\") \"c\")").unwrap();
        let flows = m
            .nodes()
            .iter()
            .filter(|n| n.form.is_flow())
            .map(|n| n.id)
            .collect::<Vec<_>>();
        assert_eq!(flows.len(), 2);
        let (outer, inner) = (flows[0], flows[1]);

        assert_eq!(m.node(outer).unwrap().mark(), "", "no arrow as a title");
        assert_eq!(m.node(inner).unwrap().mark(), "");

        // The inner flow is a circle around its own two forms, and the outer
        // arrow connects THAT circle to the third — not its innermost result.
        for operand in m.children(inner) {
            assert_eq!(m.holder(operand), Some(inner), "drawn inside its circle");
        }
        assert_eq!(m.holder(inner), Some(outer), "and itself inside the outer");
        assert_eq!(m.to_rebis().unwrap(), "(-> (-> \"a\" \"b\") \"c\")");
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
    fn a_simple_mediator_is_drawn_inside_its_square() {
        let m = Mandala::from_rebis("([\"m\"] \"a\" \"b\")").unwrap();
        let square = m.nodes().iter().find(|n| n.form == Form::Square).unwrap();

        let inner = m
            .inlined_mediator(square.id)
            .expect("a prompt mediator fits");
        assert!(m.is_inlined(inner));
        assert_eq!(m.node(inner).map(|n| n.text.as_str()), Some("m"));

        // The box grows around all three operands. The mediator participates in
        // the same compact spiral as the branches instead of being pinned over
        // the boundary's centre.
        let (half_w, half_h) = m.extent(square.id);
        assert!(half_w > NODE_R && half_h > NODE_RY, "the box grew");
        let node = m.node(inner).unwrap();
        assert!((node.x - square.x).abs() + m.extent(inner).0 < half_w);
        assert!((node.y - square.y).abs() + m.extent(inner).1 < half_h);

        assert_eq!(m.hit(node.x, node.y), Some(inner));
        // Empty space beside the packed spiral still selects the boundary.
        let corner = (
            square.x + half_w - MEDIATOR_PAD / 2.0,
            square.y + half_h - MEDIATOR_PAD / 2.0,
        );
        assert_eq!(m.hit(corner.0, corner.1), Some(square.id));

        // None of this changed the program.
        assert_eq!(m.to_rebis().unwrap(), "([\"m\"] \"a\" \"b\")");
    }

    #[test]
    fn a_square_keeps_its_mediator_and_every_branch_inside_the_box() {
        // Everything structurally owned by the square belongs inside it when
        // source is auto-drawn: the mediator flow, its operands, and both
        // branches. The boundary itself is the parent relation.
        let mut m = Mandala::from_rebis("([(-> \"x\" \"y\")] \"a\" \"b\")").unwrap();
        let square = m
            .nodes()
            .iter()
            .find(|n| n.form == Form::Square)
            .unwrap()
            .clone();

        let interior = m.interior(square.id);
        assert_eq!(
            interior.len(),
            5,
            "the flow circle, its two operands, and both branches"
        );
        assert_eq!(
            m.contained_children(square.id).len(),
            3,
            "the box directly holds the flow circle and its two branches"
        );
        assert!(interior.iter().all(|id| m.is_inlined(*id)));
        let mediator = m.inlined_mediator(square.id).expect("held mediator");
        assert_eq!(
            m.node(mediator).map(|node| &node.form),
            Some(&Form::Forward)
        );
        assert!(
            m.children(mediator)
                .into_iter()
                .all(|child| m.is_inlined(child)),
            "the mediator's children stay inside the source-written brackets"
        );

        // The box grew past its empty size to hold the mediator itself.
        let (half_w, half_h) = m.extent(square.id);
        assert!(half_w > NODE_R, "the box widened: {half_w}");
        assert!(half_h > NODE_RY, "the box heightened: {half_h}");

        // Everything inside really is inside.
        for id in &interior {
            let node = m.node(*id).unwrap();
            assert!(
                (node.x - square.x).abs() <= half_w && (node.y - square.y).abs() <= half_h,
                "{:?} escaped its box",
                node.form
            );
        }

        // Empty space inside the box selects the BOX, not whatever is nearest.
        let corner = (
            square.x + half_w - MEDIATOR_PAD / 2.0,
            square.y + half_h - MEDIATOR_PAD / 2.0,
        );
        assert_eq!(m.hit(corner.0, corner.1), Some(square.id));

        // Dragging the box carries its contents: offsets are preserved.
        let before: Vec<(NodeId, f64, f64)> = interior
            .iter()
            .map(|id| {
                let n = m.node(*id).unwrap();
                (*id, n.x - square.x, n.y - square.y)
            })
            .collect();
        m.move_group_to(square.id, square.x + 500.0, square.y - 250.0);
        let moved = m.node(square.id).unwrap().clone();
        for (id, dx, dy) in before {
            let n = m.node(id).unwrap();
            assert!((n.x - moved.x - dx).abs() < 1e-9, "interior x drifted");
            assert!((n.y - moved.y - dy).abs() < 1e-9, "interior y drifted");
        }

        // And none of it changed the program.
        assert_eq!(m.to_rebis().unwrap(), "([(-> \"x\" \"y\")] \"a\" \"b\")");
    }

    #[test]
    fn compose_contains_the_complete_source_written_subtrees() {
        let source = "(\"a\" (-> \"b\" \"c\"))";
        let mut m = Mandala::from_rebis(source).unwrap();
        let compose = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose)
            .unwrap()
            .clone();
        let interior = m.interior(compose.id);
        assert_eq!(
            interior.len(),
            4,
            "the direct prompt, the flow circle, and its two operands"
        );
        assert!(interior.iter().all(|id| m.is_inlined(*id)));
        assert_eq!(m.contained_children(compose.id).len(), 2);
        let flow = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Forward)
            .expect("flow");
        assert!(
            m.children(flow.id)
                .into_iter()
                .all(|child| m.is_inlined(child)),
            "source-written descendants remain inside the compose circle"
        );

        let (radius_x, radius_y) = m.extent(compose.id);
        assert_eq!(radius_x, radius_y, "compose must remain a true circle");
        assert!(radius_x > NODE_R, "the circle grew around its operands");
        for id in &interior {
            let node = m.node(*id).unwrap();
            let child_extent = m.extent(*id);
            let far_corner = ((node.x - compose.x).abs() + child_extent.0)
                .hypot((node.y - compose.y).abs() + child_extent.1);
            assert!(
                far_corner + MEDIATOR_PAD <= radius_x + 1e-9,
                "{:?} escaped the compose circle",
                node.form
            );
        }

        // Hit geometry follows the circle rather than its square bounds, and
        // the whole circumference offers a radial resize.
        assert_eq!(
            m.hit(compose.x + radius_x * 0.72, compose.y + radius_x * 0.72),
            None
        );
        let diagonal = radius_x / 2.0_f64.sqrt();
        let grab = m
            .border_hit(compose.x + diagonal, compose.y + diagonal, BORDER_BAND)
            .expect("the circular border resizes");
        assert_eq!(grab.id, compose.id);
        assert!(grab.wide && grab.tall);
        assert_eq!(
            m.resize_grab(compose.x + diagonal, compose.y + diagonal, BORDER_BAND),
            Some(grab)
        );

        let before = interior
            .iter()
            .map(|id| {
                let node = m.node(*id).unwrap();
                (*id, node.x - compose.x, node.y - compose.y)
            })
            .collect::<Vec<_>>();
        m.move_group_to(compose.id, compose.x + 400.0, compose.y - 150.0);
        let moved = m.node(compose.id).unwrap().clone();
        for (id, dx, dy) in before {
            let node = m.node(id).unwrap();
            assert!((node.x - moved.x - dx).abs() < 1e-9);
            assert!((node.y - moved.y - dy).abs() < 1e-9);
        }

        m.resize(compose.id, radius_x + 60.0, radius_y + 20.0);
        assert_eq!(
            m.extent(compose.id),
            (radius_x + 60.0, radius_x + 60.0),
            "manual resize changes one shared radius"
        );
        m.resize(compose.id, 1.0, 1.0);
        let restored = m.extent(compose.id);
        assert!(
            (restored.0 - radius_x).abs() < 1e-9 && (restored.1 - radius_y).abs() < 1e-9,
            "the circle cannot shrink through its contents: {restored:?}"
        );
        assert_eq!(m.to_rebis().unwrap(), source);
    }

    #[test]
    fn source_draw_nests_each_indentation_inside_the_one_that_wrote_it() {
        let source = "(\"anchor\" (~ f (x) '([,x] ,x)))";
        let mandala = Mandala::from_rebis(source).unwrap();
        let find = |want: fn(&Form) -> bool| {
            mandala
                .nodes()
                .iter()
                .find(|node| want(&node.form))
                .unwrap()
                .id
        };
        let circle = find(|form| *form == Form::Compose);
        let macro_form = find(|form| matches!(form, Form::Function(_)));
        let square = find(|form| *form == Form::Square);

        // The square is written inside the macro's parentheses, so it is drawn
        // inside the macro's circle — not flattened out into the compose that
        // merely contains the macro. Indentation nests.
        assert_eq!(
            mandala.holder(square),
            Some(macro_form),
            "the [] belongs to the macro it was written inside"
        );
        assert_eq!(
            mandala.holder(macro_form),
            Some(circle),
            "and the macro belongs to the compose it was written inside"
        );
        assert!(
            mandala
                .nodes()
                .iter()
                .filter(|node| node.id != circle)
                .all(|node| mandala.is_inlined(node.id)),
            "nothing is left loose outside a boundary"
        );
        assert_eq!(mandala.to_rebis().unwrap(), source);
    }

    #[test]
    fn structural_forms_use_their_declared_outlines() {
        assert_eq!(Form::Prompt.shape(), Shape::Hexagon);
        assert_eq!(Form::Compose.shape(), Shape::Circle);
        // The box belongs to the mediator alone: a square on the canvas can
        // only mean a mediation. The implicit top-level scope is a triangle.
        assert_eq!(Form::Square.shape(), Shape::Square);
        assert_eq!(Form::Program.shape(), Shape::Triangle);
        // A call and an input port both write their operands inside their own
        // parentheses, so both are indentations and both are drawn as the
        // circle they open, named by the notation that opened it.
        assert_eq!(Form::Call.shape(), Shape::Circle);
        assert_eq!(Form::Input.shape(), Shape::Circle);
    }

    #[test]
    fn every_form_has_a_distinct_enough_drawing() {
        // Two things distinguish a form on the canvas: its outline, and — for
        // the indentations, which all share the circle — the sigil written on
        // its ring. Every form must be told apart by that pair.
        //
        // Forward and backflow intentionally share the directional arrow glyph;
        // its head direction distinguishes them. The square belongs to the
        // mediator alone, which is why a box on the canvas is always a
        // mediation and never a scope.
        let mut seen: Vec<(Shape, String)> = Vec::new();
        let mut squares = 0;
        let mut circles = 0;
        // Built with the text the palette gives each form, because a call is
        // told from a bare `( )` by the name it is called with.
        for (_, make, text) in Form::ALL {
            let form = make();
            let shape = form.shape();
            if shape == Shape::Square {
                squares += 1;
            }
            if shape == Shape::Circle {
                circles += 1;
                assert!(
                    form.opens_indentation(),
                    "{form:?} is drawn as a circle without being an indentation"
                );
            }
            if form.is_flow() {
                // Flow shares the plain circle with compose by design: what
                // tells them apart is the arrow drawn between the two forms
                // inside it, and a circle titled with an arrow is nothing the
                // palette could build.
                continue;
            }
            let mut mandala = Mandala::new();
            let id = mandala.add(form.clone(), *text, 0.0, 0.0);
            let drawing = (shape, mandala.node(id).unwrap().mark());
            assert!(
                !seen.contains(&drawing),
                "two forms are drawn identically: {drawing:?}"
            );
            seen.push(drawing);
        }
        assert_eq!(squares, 1, "the square belongs to the mediator alone");
        assert!(circles > 1, "every parenthesised form shares the circle");
        let shapes = seen.iter().map(|(shape, _)| *shape).collect::<Vec<_>>();
        assert!(shapes.contains(&Shape::Square));
        assert!(shapes.contains(&Shape::Circle));
        assert!(shapes.contains(&Shape::Triangle));
        assert!(shapes.contains(&Shape::Hexagon));
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
            Shape::Percent,
        ] {
            assert!(!s.strokes().is_empty(), "{s:?} draws nothing");
        }
        for s in [
            Shape::Circle,
            Shape::Triangle,
            Shape::Square,
            Shape::Diamond,
        ] {
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
            Shape::Percent,
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
    fn program_triangles_use_their_outline_for_hit_testing() {
        assert_eq!(Form::Program.shape(), Shape::Triangle);
        assert!(Shape::Triangle.contains(0.0, 0.0));
        assert!(Shape::Triangle.contains(NODE_R * 0.9, NODE_RY * 0.9));
        assert!(!Shape::Triangle.contains(NODE_R * 0.8, -NODE_RY * 0.8));
        assert!(!Shape::Triangle.contains(0.0, NODE_RY + 1.0));

        for (x, y) in Shape::triangle_points() {
            assert!(Shape::Triangle.contains(x as f64 * 0.98, y as f64 * 0.98));
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
    fn sigil_shapes_and_indentation_boundaries_carry_no_caption() {
        let mut m = Mandala::new();
        let c = m.add(Form::Concat, "", 0.0, 0.0);
        let q = m.add(Form::Quote, "", 0.0, 0.0);
        let i = m.add(Form::Invert, "", 0.0, 0.0);
        let circle = m.add(Form::Compose, "", 0.0, 0.0);
        let square = m.add(Form::Square, "", 0.0, 0.0);
        assert_eq!(m.node(c).unwrap().caption(), "");
        assert_eq!(m.node(q).unwrap().caption(), "");
        assert_eq!(m.node(i).unwrap().caption(), "");
        assert_eq!(m.node(circle).unwrap().caption(), "");
        assert_eq!(m.node(square).unwrap().caption(), "");
        // An indentation writes its notation on its ring, leaving the interior
        // to the forms it holds.
        let f = m.add(Form::Function(vec!["x".into()]), "twice", 0.0, 0.0);
        assert_eq!(m.node(f).unwrap().caption(), "");
        assert_eq!(
            m.node(f).unwrap().mark(),
            "~",
            "the head alone, not its arguments"
        );
    }

    #[test]
    fn the_palette_covers_every_placeable_form() {
        // Guards against adding an Expr variant without a way to draw it.
        let names: HashSet<&str> = Form::ALL.iter().map(|(_, f, _)| f().name()).collect();
        for expected in [
            "prompt", "symbol", "import", "quote", "unquote", "invert", "square", "concat",
            "compose", "call", "macro", "program", "input", "forward", "backflow",
        ] {
            assert!(names.contains(expected), "palette is missing {expected}");
        }
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
    fn an_incomplete_flow_has_a_selectable_arrow_handle() {
        let mut m = Mandala::new();
        let arrow = m.add(Form::Forward, "", 40.0, 50.0);

        assert_eq!(m.hit(40.0, 50.0), Some(arrow));
        assert_eq!(
            m.nodes_in_rect(WorldRect::from_points((30.0, 40.0), (50.0, 60.0))),
            vec![arrow]
        );
        assert!(m.flow_result(arrow).is_none());
    }

    #[test]
    fn a_nested_flow_resolves_to_its_directional_result() {
        let mut m = Mandala::new();
        let first = m.add(Form::Prompt, "first", 0.0, 0.0);
        let second = m.add(Form::Prompt, "second", 100.0, 0.0);
        let flow = m.flow(first, second, Form::Forward).unwrap();
        assert_eq!(m.flow_result(flow), Some(second));

        let backflow = m.flow(first, second, Form::Backflow).unwrap();
        assert_eq!(m.flow_result(backflow), Some(first));
    }

    #[test]
    fn child_numbers_follow_link_order_and_can_be_changed() {
        let mut m = Mandala::new();
        let parent = m.add(Form::Compose, "", 0.0, 0.0);
        let first = m.add(Form::Prompt, "first", 1.0, 0.0);
        let second = m.add(Form::Prompt, "second", 2.0, 0.0);
        let third = m.add(Form::Prompt, "third", 3.0, 0.0);
        m.father_of(parent, first);
        m.father_of(parent, second);
        m.father_of(parent, third);

        assert_eq!(m.children(parent), vec![first, second, third]);
        assert_eq!(m.child_number(parent, first), Some(1));
        assert_eq!(m.child_number(parent, third), Some(3));
        assert!(!m.set_child_number(parent, third, 0));
        assert!(!m.set_child_number(parent, third, 4));
        assert!(m.set_child_number(parent, third, 1));
        assert_eq!(m.children(parent), vec![third, first, second]);
        assert_eq!(m.to_rebis().unwrap(), "(\"third\" \"first\" \"second\")");
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
        // The square is one indentation block. All three operands receive
        // distinct compact spiral positions within that one boundary.
        let square = m.nodes().iter().find(|n| n.form == Form::Square).unwrap();
        let contents = m.contained_children(square.id);
        assert_eq!(contents.len(), 3);
        let mut seen = HashSet::new();
        for id in contents {
            let node = m.node(id).unwrap();
            assert!(
                seen.insert((node.x.to_bits(), node.y.to_bits())),
                "two nested operands share a position"
            );
            assert!(m.is_inlined(id));
            let extent = m.extent(id);
            assert!((node.x - square.x).abs() + extent.0 < m.extent(square.id).0);
            assert!((node.y - square.y).abs() + extent.1 < m.extent(square.id).1);
        }
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
    fn format_drawing_reserves_proportional_columns_and_rows_for_resized_symbols() {
        let mut mandala = Mandala::new();
        let father = mandala.add(Form::Square, "", 0.0, 0.0);
        let upper = mandala.add(Form::Prompt, "upper", 0.0, 0.0);
        let lower = mandala.add(Form::Symbol, "lower", 0.0, 0.0);
        mandala.father_of(father, upper);
        mandala.father_of(father, lower);
        mandala.resize(father, 220.0, 90.0);
        // The children scale whole, so each one's half-extents come back in its
        // own proportions; the layout must reserve whatever that works out to.
        mandala.resize(upper, 70.0, 100.0);
        mandala.resize(lower, 80.0, 110.0);
        let upper_extent = mandala.extent(upper);
        let lower_extent = mandala.extent(lower);

        mandala.relayout();

        let father_node = mandala.node(father).unwrap();
        let upper_node = mandala.node(upper).unwrap();
        let lower_node = mandala.node(lower).unwrap();
        let child_column = upper_node.x - father_node.x;
        let widest = upper_extent.0.max(lower_extent.0);
        assert!(
            (child_column - (220.0 + COLUMN_GUTTER + widest)).abs() < 1e-9,
            "column indentation must include both resized half-widths"
        );
        assert_eq!(upper_node.x, lower_node.x);
        assert!(
            (upper_node.y - lower_node.y).abs()
                >= upper_extent.1 + lower_extent.1 + ROW_GUTTER - 1e-9,
            "resized child rows must not overlap"
        );
    }

    #[test]
    fn a_boundary_lays_its_contents_out_in_operand_order() {
        let mut m = Mandala::from_rebis("(\"one\" \"two\" \"three\" \"four\" \"five\")").unwrap();
        let circle = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose)
            .unwrap()
            .id;

        // The spiral runs in the order the source wrote, so the numbers the
        // inspector shows for child order name the forms you actually see.
        assert_eq!(
            m.contained_children(circle),
            m.children(circle),
            "the arrangement must follow operand order"
        );
        for (index, id) in m.contained_children(circle).into_iter().enumerate() {
            assert_eq!(m.child_number(circle, id), Some(index + 1));
        }

        // Holding something out of order does not reorder the drawing: where a
        // form sits on the arm is its place in the source, not the moment it
        // happened to be put inside.
        let last = m.children(circle)[4];
        m.release(last);
        m.hold(circle, last);
        assert_eq!(m.contained_children(circle), m.children(circle));

        // A form added by hand joins the end of the arm, as the newest operand.
        let added = m.nest(circle, Form::Prompt, "six").unwrap();
        let held = m.contained_children(circle);
        assert_eq!(held.last().copied(), Some(added));
        assert_eq!(m.child_number(circle, added), Some(held.len()));
    }

    #[test]
    fn formatting_takes_the_least_room_it_honestly_can() {
        let source = "(\"one\" \"two\" \"three\" \"four\" \"five\" \"six\" \"seven\" \"eight\")";
        let mut m = Mandala::from_rebis(source).unwrap();
        let circle = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose)
            .unwrap()
            .id;
        let sizes = |m: &Mandala| {
            m.contained_children(circle)
                .into_iter()
                .map(|id| m.extent(id))
                .collect::<Vec<_>>()
        };

        // Eight forms holding nothing come out at the size they are naturally
        // drawn at — no slack, and nothing inflated to match a neighbour.
        let drawn = sizes(&m);
        assert_eq!(drawn.len(), 8);
        assert!(
            drawn.iter().all(|extent| *extent == (NODE_R, NODE_RY)),
            "forms holding nothing should be at their own size: {drawn:?}"
        );

        // A form enlarged by hand is brought back to what it needs, and the
        // boundary that had grown around it closes again.
        let held = m.contained_children(circle)[3];
        m.resize(held, NODE_R * 6.0, NODE_RY * 6.0);
        let swollen = m.extent(circle).0;
        m.relayout();
        assert_eq!(sizes(&m), drawn, "formatting must undo a hand-set size");
        assert!(
            m.extent(circle).0 < swollen,
            "and close the boundary that had grown around it"
        );

        // Formatting states what the drawing is; it does not change it further.
        let settled = (sizes(&m), m.extent(circle));
        m.relayout();
        assert_eq!((sizes(&m), m.extent(circle)), settled);
    }

    #[test]
    fn format_drawing_packs_resized_contents_on_the_smallest_spiral() {
        let mut mandala =
            Mandala::from_rebis("(\"one\" \"two\" \"three\" \"four\" \"five\" \"six\")").unwrap();
        let circle = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose)
            .unwrap()
            .id;
        let items = mandala.contained_children(circle);
        assert_eq!(items.len(), 6);
        mandala.resize(items[2], 96.0, 72.0);
        mandala.relayout();

        // Every pair is separated by at least the content gutter even though
        // one symbol is much larger than the others.
        for (index, left) in items.iter().copied().enumerate() {
            let left_node = mandala.node(left).unwrap();
            let left_extent = mandala.extent(left);
            for right in items.iter().copied().skip(index + 1) {
                let right_node = mandala.node(right).unwrap();
                let right_extent = mandala.extent(right);
                assert!(
                    (left_node.x - right_node.x).abs()
                        >= left_extent.0 + right_extent.0 + CONTENT_GAP - 1e-6
                        || (left_node.y - right_node.y).abs()
                            >= left_extent.1 + right_extent.1 + CONTENT_GAP - 1e-6,
                    "spiral items {left:?} and {right:?} overlap"
                );
            }
        }

        // Relative to item zero, every point sits on the spiral's own ray: the
        // angle advances by a constant step, one continuous arm rather than a
        // phyllotaxis of separate golden rays.
        let origin = mandala.node(items[0]).unwrap();
        let mut radii = Vec::new();
        for (index, id) in items.iter().copied().enumerate().skip(1) {
            let node = mandala.node(id).unwrap();
            let delta = (node.x - origin.x, node.y - origin.y);
            let actual_angle = delta.1.atan2(delta.0);
            let expected_angle = index as f64 * SPIRAL_STEP;
            let angle_error = (actual_angle - expected_angle)
                .rem_euclid(std::f64::consts::TAU)
                .min((expected_angle - actual_angle).rem_euclid(std::f64::consts::TAU));
            assert!(angle_error < 1e-9, "item {index} left the spiral arm");
            radii.push((index, id, delta.0.hypot(delta.1)));
        }

        // And each one is as far in as its ray allows: pulled a hair closer to
        // the middle, it would run into something already placed. That is what
        // keeps the boundary around them no larger than its contents demand.
        for (index, id, radius) in radii {
            let angle = index as f64 * SPIRAL_STEP;
            let closer = radius * (1.0 - 1e-6);
            let spot = (
                origin.x + closer * angle.cos(),
                origin.y + closer * angle.sin(),
            );
            let extent = mandala.extent(id);
            let crowded = items.iter().copied().take(index).any(|earlier| {
                let node = mandala.node(earlier).unwrap();
                let other = mandala.extent(earlier);
                (spot.0 - node.x).abs() < extent.0 + other.0 + CONTENT_GAP
                    && (spot.1 - node.y).abs() < extent.1 + other.1 + CONTENT_GAP
            });
            assert!(
                crowded,
                "item {index} could have sat closer to the middle of its boundary"
            );
        }
    }

    #[test]
    fn a_unary_source_chain_becomes_one_circle_inside_another() {
        // Every `( )` in the source is one circle on the canvas, so a chain of
        // nested parentheses is a chain of nested circles rather than a row of
        // loose sigils. `'` opens nothing, so it rides at the level it is
        // written at.
        let mandala = Mandala::from_rebis("(^ '(^ \"x\"))").unwrap();
        let inverters = mandala
            .nodes()
            .iter()
            .filter(|node| node.form == Form::Invert)
            .map(|node| node.id)
            .collect::<Vec<_>>();
        assert_eq!(inverters.len(), 2);
        let (outer, inner) = (inverters[0], inverters[1]);
        assert_eq!(mandala.holder(outer), None, "the outermost stands alone");
        assert_eq!(
            mandala.holder(inner),
            Some(outer),
            "the inner `(^ …)` is drawn inside the outer one"
        );
        let quote = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Quote)
            .unwrap()
            .id;
        assert_eq!(mandala.holder(quote), Some(outer));
        let prompt = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Prompt)
            .unwrap()
            .id;
        assert_eq!(mandala.holder(prompt), Some(inner));
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
    fn fitting_a_drawing_puts_every_part_of_it_on_screen_and_centred() {
        let mandala = Mandala::from_rebis(
            "(-> ([\"pick\"] \"alpha\" \"beta\") (\"one\" \"two\" \"three\" \"four\"))",
        )
        .unwrap();
        let bounds = mandala.bounds().expect("a drawn program has bounds");
        let (width, height) = (1280.0, 720.0);
        let view = View::fitting(bounds, width, height);

        // Every symbol, complete with its extent, lands inside the window with
        // the margin intact.
        for node in mandala.nodes() {
            let extent = mandala.extent(node.id);
            for corner in [
                (node.x - extent.0, node.y - extent.1),
                (node.x + extent.0, node.y + extent.1),
            ] {
                let (sx, sy) = view.to_screen(corner.0, corner.1);
                assert!(
                    sx >= View::FIT_MARGIN - 1e-6
                        && sx <= width - View::FIT_MARGIN + 1e-6
                        && sy >= View::FIT_MARGIN - 1e-6
                        && sy <= height - View::FIT_MARGIN + 1e-6,
                    "{:?} landed off screen at ({sx}, {sy})",
                    node.id
                );
            }
        }

        // And it is centred rather than merely contained.
        let (cx, cy) = bounds.centre();
        let (sx, sy) = view.to_screen(cx, cy);
        assert!((sx - width * 0.5).abs() < 1e-6 && (sy - height * 0.5).abs() < 1e-6);
    }

    #[test]
    fn fitting_shrinks_to_the_window_but_never_magnifies_past_natural_size() {
        // A drawing far larger than the window is scaled down to it...
        let wide = WorldRect::from_points((-4000.0, -3000.0), (4000.0, 3000.0));
        let shrunk = View::fitting(wide, 800.0, 600.0);
        assert!(shrunk.zoom < 0.1, "a huge drawing must be scaled down");

        // ...but one small enough to sit inside it is shown at natural size,
        // not blown across the canvas.
        let small = WorldRect::from_points((-20.0, -15.0), (20.0, 15.0));
        let fitted = View::fitting(small, 800.0, 600.0);
        assert_eq!(fitted.zoom, 1.0);
        assert_eq!(fitted.to_screen(0.0, 0.0), (400.0, 300.0));

        // Degenerate inputs fall back rather than dividing by nothing.
        let point = WorldRect::from_points((5.0, 5.0), (5.0, 5.0));
        assert!(View::fitting(point, 800.0, 600.0).zoom.is_finite());
        assert_eq!(View::fitting(wide, 0.0, 0.0), View::new());
        assert_eq!(View::fitting(wide, f64::NAN, 600.0), View::new());
    }

    #[test]
    fn an_empty_drawing_has_no_bounds_to_frame() {
        assert_eq!(Mandala::new().bounds(), None);
        let mut m = Mandala::new();
        m.add(Form::Prompt, "a", 30.0, -12.0);
        let bounds = m.bounds().unwrap();
        assert!(bounds.width() > 0.0 && bounds.height() > 0.0);
        assert!((bounds.centre().0 - 30.0).abs() < 1e-9);
        assert!((bounds.centre().1 + 12.0).abs() < 1e-9);
    }

    #[test]
    fn the_wheel_runs_out_of_arithmetic_before_it_runs_out_of_canvas() {
        // Neither direction has an artificial stop. Scrolling in grows without
        // limit; scrolling out only stops where the transform would cease to be
        // invertible, which is some thirty orders of magnitude past the point
        // where the drawing is a single pixel.
        let mut v = View::new();
        for _ in 0..80 {
            v.zoom_at(0.0, 0.0, 1.5);
        }
        assert!(v.zoom > 1_000_000_000_000.0, "zoom stopped at {}", v.zoom);
        assert!(v.zoom.is_finite());

        let mut v = View::new();
        for _ in 0..80 {
            v.zoom_at(0.0, 0.0, 1.0 / 1.5);
        }
        assert!(v.zoom < 0.000_000_000_001, "zoom stopped at {}", v.zoom);
        // Still a usable transform at that distance: a world point maps to a
        // finite place on screen and back again.
        let (wx, wy) = v.to_world(400.0, 300.0);
        assert!(wx.is_finite() && wy.is_finite());
        let (sx, sy) = v.to_screen(wx, wy);
        assert!((sx - 400.0).abs() < 1e-6 && (sy - 300.0).abs() < 1e-6);

        for _ in 0..400 {
            v.zoom_at(0.0, 0.0, 0.5);
        }
        assert_eq!(v.zoom, View::MIN_ZOOM);
        assert!(View::MIN_ZOOM > 0.0 && View::MIN_ZOOM.is_finite());
    }

    #[test]
    fn invalid_zoom_steps_leave_the_finite_view_untouched() {
        let mut view = View::new();
        let original = view;
        for factor in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            view.zoom_at(20.0, 30.0, factor);
            assert_eq!(view, original);
        }
    }

    // ── hit testing ────────────────────────────────────────────────────────

    #[test]
    fn hit_finds_a_triangle_only_inside_its_outline() {
        let mut m = Mandala::new();
        let a = m.add(Form::Program, "", 100.0, 100.0);
        assert_eq!(m.hit(100.0, 100.0), Some(a));
        assert_eq!(m.hit(100.0 + NODE_R * 0.8, 100.0 + NODE_RY * 0.8), Some(a));
        assert_eq!(m.hit(100.0 + NODE_R * 0.8, 100.0 - NODE_RY * 0.8), None);
        assert_eq!(m.hit(100.0, 100.0 + NODE_RY + 1.0), None);
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
    fn a_squares_border_is_grabbable_and_its_middle_is_not() {
        let mut m = Mandala::new();
        let s = m.add(Form::Square, "", 0.0, 0.0);

        // The band straddles the wall: a hand aiming at the drawn line lands on
        // either side of it, and both sides must grab the wall. Reaching only
        // inwards is what made grabbing the visible edge pan the canvas instead.
        for x in [NODE_R - 1.0, NODE_R, NODE_R + 1.0] {
            let wall = m
                .border_hit(x, 0.0, BORDER_BAND)
                .unwrap_or_else(|| panic!("wall at {x}"));
            assert_eq!(wall.id, s);
            assert!(wall.wide && !wall.tall, "a side wall sizes only the width");
            assert_eq!(
                m.resize_grab(x, 0.0, BORDER_BAND).map(|grab| grab.id),
                Some(s),
                "grabbing the wall at {x} must resize, never fall through to a pan"
            );
        }

        // Far enough out is bare canvas again.
        assert!(
            m.border_hit(NODE_R + BORDER_BAND + 1.0, 0.0, BORDER_BAND)
                .is_none(),
            "past the band is canvas"
        );

        // The middle of the box is for moving it, so it is not a wall.
        assert!(
            m.border_hit(0.0, 0.0, BORDER_BAND).is_none(),
            "the middle is not a wall"
        );

        // A corner is on both walls at once, inside or out.
        for (x, y) in [(NODE_R - 1.0, NODE_RY - 1.0), (NODE_R + 1.0, NODE_RY + 1.0)] {
            let corner = m
                .border_hit(x, y, BORDER_BAND)
                .unwrap_or_else(|| panic!("corner at {x},{y}"));
            assert!(corner.wide && corner.tall, "a corner sizes both");
        }

        // Every visible symbol offers the same scale outline.
        let mut other = Mandala::new();
        let prompt = other.add(Form::Prompt, "a", 0.0, 0.0);
        assert_eq!(
            other
                .border_hit(NODE_R - 1.0, 0.0, BORDER_BAND)
                .map(|grab| grab.id),
            Some(prompt)
        );
    }

    #[test]
    fn every_visible_symbol_has_a_resizable_scale_outline() {
        for (_, make, _) in Form::ALL {
            let mut mandala = Mandala::new();
            let form = make();
            let id = mandala.add(form.clone(), form.name(), 25.0, -15.0);
            let (base_w, base_h) = mandala.extent(id);
            // Ask for more width than height. Only the square may honour the
            // two separately; every other outline takes the larger factor and
            // keeps its own proportions.
            mandala.resize(id, base_w * 1.8, base_h * 1.6);
            let (half_w, half_h) = mandala.extent(id);
            if scales_uniformly(&form) {
                assert!(
                    (half_w - base_w * 1.8).abs() < 1e-9 && (half_h - base_h * 1.8).abs() < 1e-9,
                    "{} scaled to {half_w}x{half_h}, not 1.8x its own outline",
                    form.name()
                );
                assert!(
                    (half_w / half_h - base_w / base_h).abs() < 1e-9,
                    "{} changed shape while being resized",
                    form.name()
                );
            } else {
                assert_eq!((half_w, half_h), (base_w * 1.8, base_h * 1.6));
            }
            let grab = mandala
                .resize_grab(25.0 + half_w, -15.0, BORDER_BAND)
                .unwrap_or_else(|| panic!("{} has no scale outline", form.name()));
            assert_eq!(grab.id, id);
            assert!(grab.wide);
        }
    }

    #[test]
    fn dragging_a_border_sizes_the_box_without_moving_it_or_the_source() {
        let mut m = Mandala::from_rebis("([\"m\"] \"a\" \"b\")").unwrap();
        let square = m
            .nodes()
            .iter()
            .find(|n| n.form == Form::Square)
            .unwrap()
            .clone();
        let (fit_w, fit_h) = m.extent(square.id);
        let inner = m.inlined_mediator(square.id).unwrap();
        let before = m.node(inner).map(|n| (n.x, n.y)).unwrap();

        m.resize(square.id, fit_w + 60.0, fit_h + 40.0);

        let (half_w, half_h) = m.extent(square.id);
        assert_eq!((half_w, half_h), (fit_w + 60.0, fit_h + 40.0));
        // The centre stays put, so nothing drawn inside moves with the walls.
        let after = m.node(square.id).map(|n| (n.x, n.y)).unwrap();
        assert_eq!(after, (square.x, square.y));
        assert_eq!(m.node(inner).map(|n| (n.x, n.y)), Some(before));
        // The wider box is hit out to its new wall.
        assert_eq!(m.hit(square.x + half_w - 1.0, square.y), Some(square.id));
        assert_eq!(m.hit(square.x + half_w + 1.0, square.y), None);

        // A size is presentation, exactly like a coordinate.
        assert_eq!(m.to_rebis().unwrap(), "([\"m\"] \"a\" \"b\")");
    }

    #[test]
    fn a_box_is_painted_behind_the_forms_it_covers() {
        // Drawing order alone put a late square over its neighbours, and a box
        // dragged across the canvas hid them. A box is a surface, so it goes
        // behind the other forms of its pass — and its own contents, which are a
        // separate pass, stay in front of it.
        // The box is drawn LAST, which is exactly the case drawing order gets
        // wrong: a square placed after its neighbours, or dragged over them.
        let mut m = Mandala::new();
        let under = m.add(Form::Prompt, "under", 0.0, 0.0);
        let square = m.add(Form::Square, "", 0.0, 0.0);

        let open = m.paint_order(false);
        let box_at = open.iter().position(|id| *id == square).expect("on canvas");
        let under_at = open.iter().position(|id| *id == under).expect("on canvas");
        assert!(
            box_at < under_at,
            "the box paints first so it cannot cover the form: {open:?}"
        );

        // A box's own contents are the other pass, so they stay in front of it.
        let held = Mandala::from_rebis("([\"m\"] \"a\" \"b\")").unwrap();
        let holder = held
            .nodes()
            .iter()
            .find(|n| n.form == Form::Square)
            .unwrap()
            .id;
        let inner = held
            .inlined_mediator(holder)
            .expect("the box holds its mediator");
        let canvas = held.paint_order(false);
        let inside = held.paint_order(true);
        assert!(canvas.contains(&holder));
        assert!(
            !canvas.contains(&inner),
            "the mediator is not on the canvas"
        );
        assert!(inside.contains(&inner), "it is painted inside the box");

        // Every node is painted exactly once, across both passes.
        let mut all = canvas;
        all.extend(inside);
        all.sort();
        let mut expected = held.nodes().iter().map(|n| n.id).collect::<Vec<_>>();
        expected.sort();
        assert_eq!(all, expected, "no node painted twice or dropped");
    }

    #[test]
    fn a_block_dropped_on_a_border_is_grabbed_instead_of_the_wall() {
        // Overlap never means structure, so a block may be dropped anywhere —
        // including across a box's wall. Dragging it must move the block, which
        // means the wall under it stops offering a resize.
        let mut m = Mandala::new();
        let square = m.add(Form::Square, "", 0.0, 0.0);
        let wall = (NODE_R - 1.0, 0.0);
        assert_eq!(
            m.resize_grab(wall.0, wall.1, BORDER_BAND).map(|g| g.id),
            Some(square)
        );

        let block = m.add(Form::Prompt, "loose", NODE_R, 0.0);
        assert_eq!(m.hit(wall.0, wall.1), Some(block), "the block is on top");
        assert!(
            m.border_hit(wall.0, wall.1, BORDER_BAND).is_some(),
            "the wall is still there underneath"
        );
        assert!(
            m.resize_grab(wall.0, wall.1, BORDER_BAND).is_none(),
            "but the gesture belongs to the block"
        );

        // The far wall, clear of the block, still resizes.
        assert_eq!(
            m.resize_grab(-NODE_R + 1.0, 0.0, BORDER_BAND).map(|g| g.id),
            Some(square)
        );
    }

    #[test]
    fn a_border_dragged_inward_stops_at_the_contents() {
        let mut m = Mandala::from_rebis("([(-> \"x\" \"y\")] \"a\" \"b\")").unwrap();
        let square = m
            .nodes()
            .iter()
            .find(|n| n.form == Form::Square)
            .unwrap()
            .id;
        let (fit_w, fit_h) = m.extent(square);

        // Ask for a box far smaller than what it holds.
        m.resize(square, 1.0, 1.0);
        assert_eq!(
            m.extent(square),
            (fit_w, fit_h),
            "the contents are the floor: the walls cannot pass them"
        );

        // Everything inside is still inside.
        let (half_w, half_h) = m.extent(square);
        let centre = m.node(square).map(|n| (n.x, n.y)).unwrap();
        for id in m.interior(square) {
            let node = m.node(id).unwrap();
            assert!(
                (node.x - centre.0).abs() <= half_w && (node.y - centre.1).abs() <= half_h,
                "{:?} escaped its box",
                node.form
            );
        }
    }

    #[test]
    fn recursive_square_geometry_is_finite_and_hit_testable() {
        let mut m = Mandala::new();
        let first = m.add(Form::Square, "", 0.0, 0.0);
        let second = m.add(Form::Square, "", 90.0, 0.0);
        m.father_of(first, second);
        m.father_of(second, first);

        for square in [first, second] {
            let extent = m.extent(square);
            assert!(extent.0.is_finite() && extent.1.is_finite());
            assert!(extent.0 >= NODE_R && extent.1 >= NODE_RY);
            assert!(m
                .border_hit(
                    m.node(square).unwrap().x + extent.0,
                    m.node(square).unwrap().y,
                    BORDER_BAND
                )
                .is_some());
        }
        assert!(m.hit(0.0, 0.0).is_some());
    }

    #[test]
    fn deeply_nested_square_extents_are_computed_without_recursive_growth() {
        let mut m = Mandala::new();
        let squares = (0..256)
            .map(|depth| m.add(Form::Square, "", depth as f64, 0.0))
            .collect::<Vec<_>>();
        for pair in squares.windows(2) {
            m.father_of(pair[0], pair[1]);
            m.hold(pair[0], pair[1]);
        }

        let outer = m.extent(squares[0]);
        assert!(outer.0.is_finite() && outer.1.is_finite());
        assert!(outer.0 > NODE_R && outer.1 > NODE_RY);
        // A second read comes directly from the immutable derived cache.
        assert_eq!(m.extent(squares[0]), outer);
    }

    #[test]
    fn growing_an_inner_boundary_grows_every_boundary_holding_it_by_the_same_amount() {
        // A wall dragged out inside a nest cannot be allowed to cross the wall
        // around it: each container it sits in must travel by exactly what its
        // content gained, so the nesting reads the same at every size.
        let mut m = Mandala::new();
        let outer = m.add(Form::Compose, "", 0.0, 0.0);
        let middle = m.add(Form::Square, "", 0.0, 0.0);
        let inner = m.add(Form::Compose, "", 0.0, 0.0);
        m.father_of(outer, middle);
        m.father_of(middle, inner);
        m.hold(outer, middle);
        m.hold(middle, inner);

        let before = [m.extent(outer), m.extent(middle), m.extent(inner)];
        let growth = 60.0;
        m.resize(inner, before[2].0 + growth, before[2].1 + growth);

        let after = [m.extent(outer), m.extent(middle), m.extent(inner)];
        for (index, (before, after)) in before.iter().zip(after.iter()).enumerate() {
            assert!(
                after.0 - before.0 >= growth - 1e-9 && after.1 - before.1 >= growth - 1e-9,
                "boundary {index} went from {before:?} to {after:?}, less than {growth} wider"
            );
        }
        // A box follows its content exactly; a circle has to reach the content's
        // far corner, so it travels the diagonal rather than the side.
        assert!((after[1].0 - before[1].0 - growth).abs() < 1e-9);

        // Concentric and clear: each wall still stands outside the wall within,
        // with the whole content gutter intact between them.
        assert!(after[0].0 >= after[1].0.hypot(after[1].1) + MEDIATOR_PAD - 1e-9);
        assert!(after[1].0 >= after[2].0 + MEDIATOR_PAD - 1e-9);
        assert!(after[1].1 >= after[2].1 + MEDIATOR_PAD - 1e-9);
    }

    #[test]
    fn every_outline_but_the_square_scales_whole() {
        // A square is a box: its two walls are independent, which is what lets a
        // mediator be made wide and shallow. Every other outline is a shape —
        // stretching one axis of a hexagon does not give a bigger hexagon — so
        // one factor governs both and the figure keeps its proportions.
        for (_, make, text) in Form::ALL {
            let form = make();
            if form.shape() == Shape::Arrow {
                continue; // drawn as the connection; it has no scale outline
            }
            let mut m = Mandala::new();
            let id = m.add(form.clone(), *text, 0.0, 0.0);
            let base = m.node(id).unwrap().base_extent();
            let ratio = base.0 / base.1;

            // Ask for a wildly lopsided size from both directions.
            for (want_w, want_h) in [(base.0 * 4.0, base.1), (base.0, base.1 * 4.0)] {
                m.resize(id, want_w, want_h);
                let got = m.extent(id);
                if scales_uniformly(&form) {
                    assert!(
                        (got.0 / got.1 - ratio).abs() < 1e-9,
                        "{} came out {got:?}, no longer its own shape",
                        form.name()
                    );
                    // The larger of the two requests is what it grew to.
                    let asked = (want_w / base.0).max(want_h / base.1);
                    assert!(
                        (got.0 - base.0 * asked).abs() < 1e-9,
                        "{} ignored the axis that asked for more",
                        form.name()
                    );
                } else {
                    assert_eq!(got, (want_w, want_h), "a square sizes each wall freely");
                }
            }

            // And its whole border governs both axes, so the drag that follows
            // cannot change one of them alone.
            let extent = m.extent(id);
            let grab = m
                .border_hit(extent.0, 0.0, BORDER_BAND)
                .unwrap_or_else(|| panic!("{} has no scale outline", form.name()));
            assert_eq!(
                (grab.wide, grab.tall),
                (true, scales_uniformly(&form)),
                "{} reported the wrong axes for a side grab",
                form.name()
            );
        }
    }

    #[test]
    fn a_boundary_placed_on_a_boundary_wraps_it_or_nests_inside_it() {
        let mut m = Mandala::from_rebis("(\"a\" (\"b\" \"c\"))").unwrap();
        let outer = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose && m.holder(node.id).is_none())
            .unwrap()
            .id;
        let inner = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose && m.holder(node.id) == Some(outer))
            .unwrap()
            .id;
        let was = m.child_number(outer, inner).unwrap();

        // Wrapping puts a new level ABOVE the clicked form, in its exact place.
        let around = m.wrap(inner, Form::Compose).unwrap();
        assert_eq!(m.father(inner), Some(around), "the form is now held by it");
        assert_eq!(m.father(around), Some(outer), "which took its old place");
        assert_eq!(m.child_number(outer, around), Some(was), "and its position");
        assert_eq!(m.holder(inner), Some(around), "and draws inside it");
        assert_eq!(m.holder(around), Some(outer));
        assert_eq!(m.to_rebis().unwrap(), "(\"a\" ((\"b\" \"c\")))");

        // Nesting puts one INSIDE, as its newest operand, in the next place on
        // that boundary's spiral rather than on top of what is already there.
        let within = m.nest(inner, Form::Compose, "").unwrap();
        // Nesting re-lays the boundary it went into, so every form inside —
        // the new one included — comes out clear of the rest.
        let placed = m.node(within).unwrap();
        let placed_extent = m.extent(within);
        for sibling in m.contained_children(inner) {
            if sibling == within {
                continue;
            }
            let node = m.node(sibling).unwrap();
            let extent = m.extent(sibling);
            assert!(
                (placed.x - node.x).abs() >= placed_extent.0 + extent.0 - 1e-6
                    || (placed.y - node.y).abs() >= placed_extent.1 + extent.1 - 1e-6,
                "the new form landed on top of {sibling:?}"
            );
        }
        assert_eq!(m.father(within), Some(inner));
        assert_eq!(m.holder(within), Some(inner));
        assert!(m.contained_children(inner).contains(&within));

        // Neither gesture applies to a form that opens no indentation, and
        // nothing may be nested inside one.
        let leaf = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Prompt)
            .unwrap()
            .id;
        assert!(
            m.nest(leaf, Form::Compose, "").is_none(),
            "a prompt holds nothing"
        );
        assert!(
            m.wrap(inner, Form::Prompt).is_none(),
            "a prompt is no boundary"
        );
    }

    #[test]
    fn growing_an_indentation_carries_the_forms_indented_inside_it() {
        let mut m = Mandala::from_rebis("(\"one\" \"two\" \"three\" \"four\")").unwrap();
        let circle = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose)
            .unwrap()
            .id;
        let items = m.contained_children(circle);
        let centre = m.node(circle).map(|node| (node.x, node.y)).unwrap();
        let before = m.extent(circle);
        let inside = items
            .iter()
            .map(|id| {
                let node = m.node(*id).unwrap();
                ((node.x - centre.0, node.y - centre.1), m.extent(*id))
            })
            .collect::<Vec<_>>();

        let factor = 2.5;
        m.resize_group(circle, before.0 * factor, before.1 * factor);

        assert!((m.extent(circle).0 - before.0 * factor).abs() < 1e-6);
        assert_eq!(m.node(circle).map(|node| (node.x, node.y)), Some(centre));

        // Everything inside travelled and grew by the same number, so the
        // arrangement is the drawing it was, only larger.
        for (id, ((dx, dy), extent)) in items.iter().copied().zip(inside) {
            let node = m.node(id).unwrap();
            assert!((node.x - centre.0 - dx * factor).abs() < 1e-6);
            assert!((node.y - centre.1 - dy * factor).abs() < 1e-6);
            let after = m.extent(id);
            assert!(
                (after.0 - extent.0 * factor).abs() < 1e-6
                    && (after.1 - extent.1 * factor).abs() < 1e-6,
                "content {id:?} went from {extent:?} to {after:?}, not {factor}x larger"
            );
        }

        // Closing the wall brings them back, and stops the moment the FIRST of
        // them reaches the size its own shape is naturally drawn at. They start
        // at different sizes — the spiral draws each step larger than the last —
        // so the rest come to rest still above their own natural size.
        m.resize_group(circle, NODE_R, NODE_RY);
        let mut any_at_rest = false;
        for id in items.iter().copied() {
            let node = m.node(id).unwrap();
            let (base, back) = (node.base_extent(), m.extent(id));
            assert!(
                back.0 >= base.0 - 1e-9 && back.1 >= base.1 - 1e-9,
                "content {id:?} shrank to {back:?}, below its natural {base:?}"
            );
            any_at_rest |= (back.0 - base.0).abs() < 1e-9 && (back.1 - base.1).abs() < 1e-9;
        }
        assert!(
            any_at_rest,
            "closing the wall must run down to the first form's natural size"
        );
    }

    #[test]
    fn a_scaled_indentation_never_overlaps_what_it_holds() {
        let mut m = Mandala::from_rebis("(\"one\" \"two\" \"three\" \"four\" \"five\")").unwrap();
        let circle = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose)
            .unwrap()
            .id;
        let items = m.contained_children(circle);

        for request in [40.0, 90.0, 260.0, 900.0, 30.0] {
            m.resize_group(circle, request, request);
            let radius = m.extent(circle).0;
            let centre = m.node(circle).map(|node| (node.x, node.y)).unwrap();
            for (index, left) in items.iter().copied().enumerate() {
                let left_node = m.node(left).unwrap();
                let left_extent = m.extent(left);
                let far = (left_node.x - centre.0).abs() + left_extent.0;
                let high = (left_node.y - centre.1).abs() + left_extent.1;
                assert!(
                    far.hypot(high) <= radius + 1e-6,
                    "at {request} the content {left:?} crossed its own boundary"
                );
                for right in items.iter().copied().skip(index + 1) {
                    let right_node = m.node(right).unwrap();
                    let right_extent = m.extent(right);
                    assert!(
                        (left_node.x - right_node.x).abs() >= left_extent.0 + right_extent.0 - 1e-6
                            || (left_node.y - right_node.y).abs()
                                >= left_extent.1 + right_extent.1 - 1e-6,
                        "at {request} the contents {left:?} and {right:?} overlap"
                    );
                }
            }
        }
    }

    #[test]
    fn dragging_a_wall_out_scales_the_group_by_where_it_lands_not_by_how_slowly() {
        // A drag arrives as a request per frame, so the gesture must compose: a
        // hundred small steps and one large one have to leave the drawing in the
        // same place, or the result would depend on the frame rate.
        let source = "([\"m\"] \"a\" \"b\" \"c\")";
        let boundary = |m: &Mandala| {
            m.nodes()
                .iter()
                .find(|node| node.form == Form::Square)
                .unwrap()
                .id
        };

        let mut swept = Mandala::from_rebis(source).unwrap();
        let square = boundary(&swept);
        let start = swept.extent(square);
        let steps = 100;
        for step in 1..=steps {
            let reached = 1.0 + 2.0 * f64::from(step) / f64::from(steps);
            swept.resize_group(square, start.0 * reached, start.1 * reached);
        }

        let mut straight = Mandala::from_rebis(source).unwrap();
        let same = boundary(&straight);
        straight.resize_group(same, start.0 * 3.0, start.1 * 3.0);

        for id in straight.contained_children(same) {
            let one = swept.node(id).unwrap();
            let other = straight.node(id).unwrap();
            assert!(
                (one.x - other.x).abs() < 1e-6 && (one.y - other.y).abs() < 1e-6,
                "content {id:?} did not land in the same place"
            );
            let (swept_extent, straight_extent) = (swept.extent(id), straight.extent(id));
            assert!(
                (swept_extent.0 - straight_extent.0).abs() < 1e-6
                    && (swept_extent.1 - straight_extent.1).abs() < 1e-6,
                "content {id:?} ended {swept_extent:?} swept and {straight_extent:?} straight"
            );
        }
    }

    #[test]
    fn a_boundary_pinned_for_a_drag_survives_releasing_its_content_and_then_lets_go() {
        // Dragging a piece releases it so its boundary does not chase it out.
        // On its own that also collapsed the boundary to nothing under a piece
        // merely being rearranged inside it, which is why the wall is pinned at
        // its current size first and handed back to its contents afterwards.
        let mut m = Mandala::new();
        let circle = m.add(Form::Compose, "", 0.0, 0.0);
        let inside = m.add(Form::Prompt, "a", 120.0, 0.0);
        m.father_of(circle, inside);
        m.hold(circle, inside);
        let drawn = m.extent(circle);
        assert!(drawn.0 > NODE_R, "the circle grew to hold its content");
        assert_eq!(m.hand_size(circle), None, "nothing has been sized by hand");

        m.resize(circle, drawn.0, drawn.1);
        m.release(inside);
        assert_eq!(
            m.extent(circle),
            drawn,
            "the pinned wall stays where it was"
        );

        m.set_size(circle, None);
        assert_eq!(
            m.extent(circle),
            (NODE_R, NODE_R),
            "letting go returns the wall to its contents"
        );
    }

    #[test]
    fn geometry_cache_is_invalidated_by_presentation_and_content_edits() {
        let mut m = Mandala::new();
        let square = m.add(Form::Square, "", 0.0, 0.0);
        let mediator = m.add(Form::Prompt, "m", 0.0, 0.0);
        m.father_of(square, mediator);
        m.hold(square, mediator);
        let initial = m.extent(square);

        m.move_to(mediator, 200.0, 0.0);
        let moved = m.extent(square);
        assert!(moved.0 > initial.0);

        m.release(mediator);
        assert_eq!(m.extent(square), (NODE_R, NODE_RY));
    }

    #[test]
    fn square_expansion_pushes_unrelated_forms_beyond_its_border() {
        let mut m = Mandala::new();
        let square = m.add(Form::Square, "", 0.0, 0.0);
        let loose = m.add(Form::Prompt, "loose", 100.0, 0.0);
        let mediator = m.add(Form::Prompt, "mediator", 200.0, 0.0);
        m.father_of(square, mediator);
        m.hold(square, mediator);

        m.make_room_for_container(square);

        let square_node = m.node(square).unwrap();
        let loose_node = m.node(loose).unwrap();
        let square_extent = m.extent(square);
        let loose_extent = m.extent(loose);
        assert!(
            (loose_node.x - square_node.x).abs() > square_extent.0 + loose_extent.0
                || (loose_node.y - square_node.y).abs() > square_extent.1 + loose_extent.1,
            "the loose form must remain visibly outside the structural box"
        );
        assert!(m.is_inlined(mediator));
        assert!(!m.is_inlined(loose));
    }

    #[test]
    fn compose_expansion_keeps_overlapping_loose_forms_outside() {
        let mut m = Mandala::new();
        let compose = m.add(Form::Compose, "", 0.0, 0.0);
        let loose = m.add(Form::Prompt, "loose", 100.0, 0.0);
        let operand = m.add(Form::Prompt, "operand", 220.0, 0.0);
        m.father_of(compose, operand);
        m.hold(compose, operand);

        m.make_room_for_container(compose);

        let circle = m.node(compose).unwrap();
        let loose_node = m.node(loose).unwrap();
        let radius = m.extent(compose).0;
        let loose_extent = m.extent(loose);
        assert!(
            (loose_node.x - circle.x).abs() > radius + loose_extent.0
                || (loose_node.y - circle.y).abs() > radius + loose_extent.1,
            "a loose form must not appear to become part of the compose circle"
        );
        assert!(m.is_inlined(operand));
        assert!(!m.is_inlined(loose));
    }

    #[test]
    fn hit_misses_empty_canvas() {
        let mut m = Mandala::new();
        m.add(Form::Prompt, "a", 0.0, 0.0);
        assert_eq!(m.hit(500.0, 500.0), None);
    }
}
