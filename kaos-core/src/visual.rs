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
    /// `(? a b …)` — a flashback: answer from the record, not from a model.
    Flashback,
    /// `(! a)` — a dream: this answer is kept beyond the run.
    Dream,
    /// `<>` — source: the program itself.
    ///
    /// An atom, not a boundary: it takes no operands, because it names
    /// something the runtime already holds rather than composing anything.
    Source,
    /// `(>< a b …)` — meta: a prompt whose answer is a program, and runs.
    Meta,
    /// `|a b …|` — the numeric plane: quantity, and arithmetic.
    ///
    /// The other pole of the emblem. A boundary like the compose circle, drawn
    /// as the opposite side rather than as a fourth exotic outline.
    Numeric,
    /// `(* topic body)` — supersede: correct what the record believes.
    Supersede,
    /// `(@ check body)` — an invariant over every arrow inside.
    Invariant,
    /// `{a b …}` — an imaginary space: work that leaves no evidence.
    ///
    /// A compose boundary standing on the imaginary axis. Everything drawn
    /// inside runs and is traced; only the space's own answer becomes evidence
    /// when it crosses the boundary.
    Imaginary,
    /// `(= name value body)` — run the value once and name its answer.
    Bind,
    /// `(&: source)` — obtain the value of a source the program named.
    Load,
    /// `(+ context body)` — frame everything in the body with the context.
    Context,
    /// `(/ selector body)` — route every model call in the body.
    ///
    /// Routing used to be a postfix suffix, unwrapped into [`Node::model`] and
    /// so invisible on the canvas: a drawing could not show which part of a
    /// program ran on which model. As a form it is a circle wearing its
    /// selector, with the routed subtree drawn inside it.
    ///
    /// The per-node [`Node::model`] override stays — it is how the panel pins
    /// one form without wrapping it — so a program has both a drawn scope and
    /// a per-form exception, exactly as it has both `+` and a prompt's own
    /// words.
    Route,
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
        // In the order the language's own punctuation is listed — the same
        // sequence the terminal app shows across its top bar, so a reader who
        // learns the alphabet in one front end finds it in the same order in
        // the other. Indentation leads, because a drawing is nesting first.
        // Forms written with no punctuation of their own follow at the end.
        ("( ) compose", || Form::Compose, ""),
        ("[] square", || Form::Square, ""),
        ("{} imaginary", || Form::Imaginary, ""),
        ("| numeric", || Form::Numeric, ""),
        ("~ macro", || Form::Function(vec!["x".into()]), "f"),
        ("# import", || Form::Import, "std/flow"),
        ("' quote", || Form::Quote, ""),
        (", unquote", || Form::Unquote, ""),
        ("$ concat", || Form::Concat, ""),
        ("? flashback", || Form::Flashback, ""),
        ("! dream", || Form::Dream, ""),
        ("* supersede", || Form::Supersede, ""),
        ("= bind", || Form::Bind, "n"),
        ("& input", || Form::Input, "input"),
        ("&: load", || Form::Load, ""),
        ("+ context", || Form::Context, ""),
        ("@ invariant", || Form::Invariant, ""),
        ("/ route", || Form::Route, "ollama:qwen3:4b"),
        ("% binary gate", || Form::Conditional, ""),
        ("^ invert", || Form::Invert, ""),
        ("<> source", || Form::Source, ""),
        (">< meta", || Form::Meta, ""),
        ("→ forward", || Form::Forward, ""),
        ("← backflow", || Form::Backflow, ""),
        ("⬡ prompt", || Form::Prompt, "prompt"),
        ("◇ symbol", || Form::Symbol, "x"),
        ("call", || Form::Call, "f"),
        ("program", || Form::Program, ""),
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
                | Form::Imaginary
                | Form::Numeric
                | Form::Meta
                | Form::Supersede
                | Form::Invariant
                | Form::Concat
                | Form::Flashback
                | Form::Dream
                | Form::Bind
                | Form::Load
                | Form::Context
                | Form::Route
                | Form::Invert
                | Form::Conditional
                | Form::Function(_)
                | Form::Input
                | Form::Call
                // The program is the outermost boundary there is. It used to hold
                // nothing, which left every top-level form loose on the page and the
                // program itself a small mark floating beside them — a container that
                // contained nothing.
                | Form::Program
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

    /// Whether the form is written as a mark immediately before its one operand.
    ///
    /// `'form` and `,value` open no parentheses and take no place of their own
    /// in the source — they are read as part of the thing they precede. Drawing
    /// them as separate forms scattered among their neighbours lost exactly the
    /// relation that adjacency was carrying, so they are drawn where they are
    /// written: on the front of their operand.
    #[must_use]
    pub fn is_prefix_sigil(&self) -> bool {
        matches!(self, Form::Quote | Form::Unquote)
    }

    /// The character the prefix is written with.
    #[must_use]
    pub fn prefix_sigil(&self) -> &'static str {
        match self {
            Form::Quote => "'",
            Form::Unquote => ",",
            _ => "",
        }
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
            // Punctuation draws itself. A diamond is what a name the AUTHOR
            // wrote looks like, and `@` is not one — it is the language's own
            // mark for the program, so it wears its own glyph exactly as `#`,
            // `'` and `,` do.
            Form::Source => Shape::Source,
            Form::Import => Shape::Hash,
            Form::Quote => Shape::Quote,
            Form::Unquote => Shape::Comma,
            Form::Square => Shape::Square,
            // The one boundary that is neither circle nor box: four sides, each
            // drawn as a brace curve, so an imaginary space is recognisable as a
            // boundary at a glance and as a DIFFERENT boundary on a second look.
            Form::Imaginary => Shape::Brace,
            // Every parenthesised form is one indentation, and every indentation
            // is one circle. The sigil it was written with becomes that circle's
            // mark; see `Node::mark`.
            Form::Compose
            | Form::Concat
            | Form::Flashback
            | Form::Dream
            | Form::Bind
            | Form::Load
            | Form::Context
            | Form::Route
            | Form::Meta
            | Form::Numeric
            | Form::Supersede
            | Form::Invariant
            | Form::Function(_)
            | Form::Invert
            | Form::Call
            | Form::Input
            | Form::Conditional
            // A program is a compose that happens to be outermost. Nothing in the
            // language distinguishes them — every evaluator and runtime site reads
            // `Program(items) | Compose(items)` as one case — and the only difference
            // is that the outermost level prints without its parentheses. A shape of
            // its own claimed a form the language does not have.
            | Form::Program => Shape::Circle,
            // A flow is the one form that is not a shape at all: it is drawn as
            // the arrow BETWEEN the two forms it routes, so it claims no outline
            // and no slot. A circle around it said the arrow was a thing rather
            // than a connection, and nothing in the palette can build a circle
            // whose title is an arrow.
            Form::Forward | Form::Backflow => Shape::Arrow,
        }
    }

    pub fn arity(&self) -> Arity {
        match self {
            Form::Prompt | Form::Symbol | Form::Source | Form::Import => Arity::Exactly(0),
            Form::Quote | Form::Unquote | Form::Invert | Form::Function(_) | Form::Input => {
                Arity::Exactly(1)
            }
            Form::Forward | Form::Backflow => Arity::Exactly(2),
            Form::Square => Arity::AtLeast(2),
            Form::Conditional => Arity::Exactly(3),
            Form::Program => Arity::AtLeast(2),
            Form::Concat
            | Form::Flashback
            | Form::Meta
            | Form::Numeric
            | Form::Compose
            | Form::Imaginary => Arity::AtLeast(1),
            // A dream keeps ITS answer, and two operands are two answers.
            Form::Dream => Arity::Exactly(1),
            // The value, then the scope that uses it.
            Form::Bind | Form::Supersede | Form::Invariant => Arity::Exactly(2),
            // `&` obtains nothing but itself; `&:` names one source; `+` takes
            // a framing and the scope it frames.
            Form::Load => Arity::Exactly(1),
            Form::Context => Arity::Exactly(2),
            // The selector is the form's own text, so only the body is a child.
            Form::Route => Arity::Exactly(1),
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
                | Form::Bind
                | Form::Route
                | Form::Input
        )
    }

    pub fn name(&self) -> &'static str {
        match self {
            Form::Prompt => "prompt",
            Form::Symbol => "symbol",
            Form::Source => "source",
            Form::Meta => "meta",
            Form::Numeric => "numeric",
            Form::Supersede => "supersede",
            Form::Invariant => "invariant",
            Form::Import => "import",
            Form::Quote => "quote",
            Form::Unquote => "unquote",
            Form::Invert => "invert",
            Form::Forward => "forward",
            Form::Backflow => "backflow",
            Form::Square => "square",
            Form::Conditional => "binary gate",
            Form::Concat => "concat",
            Form::Flashback => "flashback",
            Form::Dream => "dream",
            Form::Bind => "bind",
            Form::Load => "load",
            Form::Context => "context",
            Form::Route => "route",
            Form::Compose => "compose",
            Form::Imaginary => "imaginary",
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
    /// `◇` — a symbol: a name rather than a literal.
    Diamond,
    /// `[]` — the mediator square, and nothing else. The box belongs to the
    /// one form whose notation is a box, so a square on the canvas always
    /// means a mediation.
    Square,
    /// `{}` — a box whose four sides are each drawn as a brace curve.
    ///
    /// A boundary like the square and the circle, and unmistakably neither:
    /// the pinched sides read as the delimiter the form is written with.
    Brace,
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
    /// `<>` — source: the program itself.
    Source,
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
            | Shape::Source
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
            Shape::Square | Shape::Brace | Shape::Hexagon | Shape::Arrow => (NODE_R, NODE_RY),
        }
    }

    /// The strokes that draw this shape's sigil, or empty for the shapes that
    /// are outlines ([`Shape::Circle`],
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
            // An at-sign: the inner ring, then the outer ring opened at the
            // lower right, then the tail that opening becomes.
            Shape::Source => &[
                Stroke::Cubic([(6.0, 2.0), (6.0, -1.3), (3.3, -4.0), (0.0, -4.0)]),
                Stroke::Cubic([(0.0, -4.0), (-3.3, -4.0), (-6.0, -1.3), (-6.0, 2.0)]),
                Stroke::Cubic([(-6.0, 2.0), (-6.0, 5.3), (-3.3, 8.0), (0.0, 8.0)]),
                Stroke::Cubic([(0.0, 8.0), (3.3, 8.0), (6.0, 5.3), (6.0, 2.0)]),
                Stroke::Poly(&[(6.0, -4.0), (6.0, 6.0)]),
                Stroke::Cubic([(6.0, 6.0), (9.0, 9.0), (15.0, 8.0), (15.0, 2.0)]),
                Stroke::Cubic([(15.0, 2.0), (15.0, -9.0), (7.0, -16.0), (-2.0, -16.0)]),
                Stroke::Cubic([(-2.0, -16.0), (-11.0, -16.0), (-17.0, -8.0), (-17.0, 1.0)]),
                Stroke::Cubic([(-17.0, 1.0), (-17.0, 10.0), (-10.0, 17.0), (-1.0, 17.0)]),
                Stroke::Poly(&[(-1.0, 17.0), (7.0, 17.0)]),
            ],
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
            | Shape::Square
            | Shape::Brace
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
    /// A size this boundary must not shrink BELOW, set when a hand gesture inside it
    /// would otherwise have pulled its wall in.
    ///
    /// Distinct from `size`, which is a size someone chose for this form: a floor is
    /// a size someone chose for something else and this form merely has to respect.
    /// A boundary's size is derived from what it holds, so shrinking one content
    /// dragged its container in after it, and that container's container, out to the
    /// page — one circle made smaller rearranged the whole drawing. Growing still
    /// happens freely, because a wall has to keep containing what it holds.
    ///
    /// Cleared by formatting, which is the gesture that hands a boundary back to its
    /// contents. Presentation only, like `size`.
    pub floor: Option<(f64, f64)>,
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

    /// The written head of an indentation, on its ring.
    ///
    /// A circle is one `( )`, and this is the line that opened it, whole:
    /// `~ greet (name)` for a macro, `& port` for an input, the callee's name
    /// for a call, `$` for a concatenation that has nothing else to its head.
    /// It is drawn on the boundary rather than inside it, so it names the
    /// indentation without standing among the forms indented within.
    ///
    /// The head and *only* the head — never the operands, which are the forms
    /// drawn inside. A macro's name and parameters are not its body; they are
    /// what the reader needs in order to know which macro they are looking at
    /// and how to call it, and a ring reading `~` beside another ring reading
    /// `~` told them neither. The panel is where the rest of a form lives, but
    /// the identity of a boundary belongs on the boundary.
    ///
    /// The ring's label is measured, so a long head widens the room the
    /// boundary reserves rather than running through its neighbour — see
    /// `mark_band`.
    ///
    /// Empty for a bare `( )` compose, which was opened by nothing but the
    /// parenthesis, and for every form that is not an indentation.
    #[must_use]
    pub fn mark(&self) -> String {
        match &self.form {
            // A circle wearing its own sigil, exactly as `$` and `?` do.
            Form::Meta => "><".into(),
            Form::Numeric => "|".into(),
            Form::Supersede => "*".into(),
            Form::Invariant => "@".into(),
            Form::Concat => "$".into(),
            Form::Flashback => "?".into(),
            Form::Dream => "!".into(),
            Form::Load => "&:".into(),
            Form::Context => "+".into(),
            // `(/ selector body)` — the selector is the whole point of the form.
            Form::Route => match self.text.trim() {
                "" => "/".into(),
                selector => format!("/ {selector}"),
            },
            // `(= n value body)` — the name is the whole point of the form.
            Form::Bind => match self.text.trim() {
                "" => "=".into(),
                name => format!("= {name}"),
            },
            Form::Invert => "^".into(),
            Form::Conditional => "%".into(),
            // `(& port body)` — the port is the whole point of the form.
            Form::Input => match self.text.trim() {
                "" => "&".into(),
                port => format!("& {port}"),
            },
            // `(~ name (p …) body)`, exactly as it is written, parameters
            // included: an empty list is written `()` because that is what the
            // source says, and a macro of no arguments reads differently from
            // one whose arguments you cannot see.
            Form::Function(params) => {
                let (name, params) = (self.text.trim(), params.join(" "));
                if name.is_empty() {
                    format!("~ ({params})")
                } else {
                    format!("~ {name} ({params})")
                }
            }
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
pub const CONTENT_GAP: f64 = 10.0;

/// How much of a form's text the drawing writes inside it.
///
/// A drawing is a map, and a map is read at a glance — which is why one token
/// per form is the default. But a map is also a thing you print and hand to
/// someone, and then the panel holding the rest of the text is not in the room.
/// The two readings want different drawings, so they are a setting rather than
/// an argument.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Legend {
    /// One token per form; the panel beside the canvas holds the rest.
    #[default]
    Token,
    /// Every form's whole text, with its outline grown until the text fits
    /// inside at a readable size.
    Whole,
}

/// How the sizes of forms relate across a drawing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Sizing {
    /// Each level is measured on its own: brothers share the widest size among
    /// them, and a boundary's contents are grown to fill the wall they are
    /// given. Sizes then say which level you are looking at, which is what
    /// makes a nested drawing readable *while you move through it*.
    ///
    /// It is not the default because of what it costs. Handing every boundary
    /// at a level the width of the widest one multiplies outward: measured on
    /// a page of macros, one holding a paragraph and the rest holding a symbol
    /// each, it doubled the drawing's span and quadrupled its area, and all of
    /// that was empty.
    ByLevel,
    /// One size for every form in the drawing, at every depth, and a boundary
    /// exactly as large as what it holds.
    ///
    /// The nested reading grows the page faster than it grows the symbols, so
    /// a whole program fitted to a window leaves its innermost forms below a
    /// pixel and the only way to read them is to travel. Evening the sizes
    /// spends nothing on saying which level a form is written at — indentation
    /// already says that, by being drawn inside something — and spends what it
    /// saves on the page being small enough to read at one magnification.
    /// Which is also what a printed sheet needs, since a sheet cannot be
    /// zoomed.
    #[default]
    Even,
}

/// The size a caption is written at inside its own outline, in world units.
///
/// One number for the whole app: the canvas draws at this size (scaled by the
/// view), and [`Legend::Whole`] sizes outlines so their text fits *at* it.
pub const LABEL_HEIGHT: f64 = 11.0;

/// One monospace character's width, as a fraction of its height.
///
/// An estimate, and deliberately the same estimate the canvas and the PDF
/// exporter lay text out with — a form sized by one metric and typeset by
/// another would either overflow its outline or rattle around inside it.
pub const LABEL_ADVANCE: f64 = 0.62;

/// The height of one line of a caption, as a multiple of the type size.
pub const LABEL_LEADING: f64 = 1.16;

/// The longest line a whole caption is broken at, in characters.
///
/// A paragraph set as one line makes a form wider than the page; set too
/// narrow it becomes a column of two-letter fragments. Twenty-four is about
/// where a long prompt comes out square, which is the shape that costs a
/// spiral the least room.
pub const LABEL_WRAP: usize = 24;

/// How many times a boundary's interior is grown towards its wall.
///
/// A fit is its contents plus a pad, so scaling the contents by the ratio the
/// wall asks for leaves the pad's share behind — a small remainder, divided
/// again by the same ratio on each pass. Three take the worst case on the
/// collection (a bare symbol under a wall five times its size) to under a
/// thousandth of the radius, which is well below a pixel at any readable zoom.
const FILL_PASSES: usize = 3;

/// How close to its wall a boundary's contents count as touching it.
///
/// The stop condition for [`Mandala::fill_boundary`], and with it the guarantee
/// that formatting twice is formatting once: below this the pass is skipped
/// outright, so a settled drawing is left byte-for-byte alone.
const FILL_TOLERANCE: f64 = 1e-3;

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
    /// How much of each form's text is written inside it. Presentation, like
    /// positions: it changes the drawing and never the generated Rebis.
    legend: Legend,
    /// How sizes relate across the drawing. Also presentation, and also part of
    /// the document rather than of the window — it decides the arrangement
    /// [`Self::relayout`] produces, so undo has to be able to take it back with
    /// the arrangement it produced.
    sizing: Sizing,
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
            legend: self.legend,
            sizing: self.sizing,
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
            legend: Legend::default(),
            sizing: Sizing::default(),
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

        // Under `Legend::Whole` a form's own text is a floor on its size, so
        // it belongs here with the shape's natural extent rather than in the
        // renderer: everything downstream — a boundary closing on its contents,
        // the spiral packing them apart, the framing that fits the page —
        // measures through these two tables and would otherwise lay out one
        // drawing while the canvas painted another.
        let legend = mandala.legend;
        let natural = |node: &Node| {
            let text = caption_floor(node, legend);
            move |base: (f64, f64)| (base.0.max(text.0), base.1.max(text.1))
        };
        let mut fits = mandala
            .nodes
            .iter()
            .map(|node| (node.id, natural(node)(default_extent(node))))
            .collect::<HashMap<_, _>>();
        let mut extents = mandala
            .nodes
            .iter()
            .map(|node| (node.id, natural(node)(base_extent(node))))
            .collect::<HashMap<_, _>>();

        while let Some(component) = ready.pop_front() {
            for container_index in &members[component] {
                let container_id = containers[*container_index];
                let Some(container) = mandala.node(container_id) else {
                    continue;
                };
                let mut fit = natural(container)(default_extent(container));
                for inner_id in &interiors[*container_index] {
                    let Some(inner) = mandala.node(*inner_id) else {
                        continue;
                    };
                    let held = natural(inner);
                    let child_extent = container_indices.get(inner_id).map_or_else(
                        || held(base_extent(inner)),
                        |inner_index| {
                            if components[*inner_index] == component {
                                held(base_extent(inner))
                            } else {
                                extents
                                    .get(inner_id)
                                    .copied()
                                    .unwrap_or_else(|| held(base_extent(inner)))
                            }
                        },
                    );
                    // The room a held form needs, not merely the outline it
                    // draws: a form's sigil stands outside its own outline, and a
                    // boundary closing on the outline alone cut straight through
                    // the mark. The band is sized from the mark that will
                    // actually be drawn — a flat one was right for an unresized
                    // circle and short by up to six times for a resized one, so a
                    // scaled-up `~` disappeared under its container's wall.
                    let band = mark_band(inner, &mandala.written_prefix(*inner_id), child_extent);
                    let child_extent = (child_extent.0 + band, child_extent.1 + band);
                    let far_x = (inner.x - container.x).abs() + child_extent.0;
                    let far_y = (inner.y - container.y).abs() + child_extent.1;
                    if is_circular_boundary(&container.form) {
                        // A circle must reach the farthest point of what it holds,
                        // which is the centre distance plus the radius of the
                        // smallest circle containing that form — never the corner of
                        // its bounding box. Only a square HAS corners to reach for;
                        // every other shape on the canvas is inscribed in its own
                        // bounds, and reaching for a round form's phantom corner
                        // inflated each nesting level by a further √2.
                        let reach = (inner.x - container.x).hypot(inner.y - container.y)
                            + circumscribed_reach(inner.shape(), child_extent);
                        // Plus room for the boundary's OWN mark, which hangs inside
                        // its ring. Two passes because the mark's size is read from
                        // the radius it is helping to set — it grows as the fourth
                        // root of the area, so one correction settles it.
                        let mut radius = reach + MEDIATOR_PAD;
                        for _ in 0..2 {
                            radius = reach
                                + MEDIATOR_PAD
                                + mark_inward(
                                    container,
                                    &mandala.written_prefix(container_id),
                                    (radius, radius),
                                );
                        }
                        fit = (fit.0.max(radius), fit.1.max(radius));
                    } else {
                        let mut inward = 0.0;
                        for _ in 0..2 {
                            inward = mark_inward(
                                container,
                                &mandala.written_prefix(container_id),
                                (far_x + MEDIATOR_PAD, far_y + MEDIATOR_PAD + inward),
                            );
                        }
                        fit.0 = fit.0.max(far_x + MEDIATOR_PAD);
                        fit.1 = fit.1.max(far_y + MEDIATOR_PAD + inward);
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

/// The room a text-bearing outline has inside it, as multiples of its own
/// half-extents.
///
/// The largest conservative rectangle that stays within the shape: a circle
/// gives less of its bounding box than a square does, and a diamond less
/// again. Expressed as factors rather than as a rectangle so that it reads
/// both ways — the renderer multiplies to find the room it may set type in,
/// and [`Legend::Whole`] divides to find the outline a given block of type
/// needs. One table, so the two can never drift apart and leave text hanging
/// over a wall.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CaptionArea {
    /// Sideways shift of the text's centre, as a fraction of the half-width.
    pub offset: f64,
    /// Usable width, as a multiple of the half-width.
    pub width: f64,
    /// Usable height, as a multiple of the half-height.
    pub height: f64,
}

/// The interior [`CaptionArea`] of a shape.
#[must_use]
pub const fn caption_area(shape: Shape) -> CaptionArea {
    const fn area(offset: f64, width: f64, height: f64) -> CaptionArea {
        CaptionArea {
            offset,
            width,
            height,
        }
    }
    match shape {
        Shape::Circle => area(0.0, 1.34, 1.34),
        Shape::Diamond => area(0.0, 1.12, 1.12),
        Shape::Square => area(0.0, 1.72, 1.72),
        Shape::Parallelogram => area(0.0, 1.42, 1.54),
        // The `&` glyph stands to the left of its own text.
        Shape::Amp => area(0.10, 1.18, 1.54),
        Shape::Hexagon => area(0.0, 1.50, 1.55),
        _ => area(0.0, 2.0, 2.0),
    }
}

/// Break a caption into the lines a drawing writes it on.
///
/// Nothing is deleted or replaced: breaks prefer existing whitespace, and a
/// word longer than the line is split rather than elided, so an unbroken path
/// or model selector still appears in full.
#[must_use]
pub fn wrap_caption(text: &str, columns: usize) -> Vec<String> {
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

/// The smallest outline that holds this form's whole caption at reading size.
///
/// Zero under [`Legend::Token`], and zero for anything that writes no caption —
/// an indentation wears its head on its ring, which is measured separately.
///
/// The inverse of [`caption_area`], through the same character metrics the
/// renderer uses. Shapes that keep their proportions are scaled whole, so a
/// hexagon holding a paragraph is a larger hexagon rather than a letterbox.
fn caption_floor(node: &Node, legend: Legend) -> (f64, f64) {
    if legend == Legend::Token {
        return (0.0, 0.0);
    }
    let caption = node.caption();
    if caption.trim().is_empty() {
        return (0.0, 0.0);
    }
    let lines = wrap_caption(&caption, LABEL_WRAP);
    #[allow(clippy::cast_precision_loss)]
    let columns = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    #[allow(clippy::cast_precision_loss)]
    let rows = lines.len().max(1) as f64;
    let text = (
        columns * LABEL_HEIGHT * LABEL_ADVANCE,
        rows * LABEL_HEIGHT * LABEL_LEADING,
    );
    let area = caption_area(node.shape());
    let want = (text.0 / area.width, text.1 / area.height);
    if scales_uniformly(&node.form) {
        // One factor, from whichever axis is tighter: a shape is a shape at
        // every size, and stretching one to fit a paragraph would make it a
        // different figure.
        let base = node.base_extent();
        let scale = (want.0 / base.0.max(f64::EPSILON))
            .max(want.1 / base.1.max(f64::EPSILON))
            .max(1.0);
        (base.0 * scale, base.1 * scale)
    } else {
        want
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

/// The room a ring label needs, in world units.
///
/// A mark is written centred on its boundary's outline, so half of it stands
/// outside — and nothing reserved that room. Neighbours were packed against the
/// wall the label sits on, and the framing that fits a drawing to the window
/// measured only outlines, so the mark on the outermost circle fell off the
/// edge.
///
/// Reserved on every circle, marked or not. A bare `( )` wearing nothing then
/// takes the same room as a `$` beside it, which is what makes a row of them
/// read as a row rather than as shapes that happen to be near each other.
///
/// It is room *around* the outline, never part of it: the circle drawn and the
/// circle clicked are the same size as before, and only what has to leave space
/// for the label — packing, the boundary above, and the framing that fits a
/// drawing to the window — reads [`Mandala::footprint`] instead of the extent.
pub const LABEL_BAND: f64 = 12.0;

/// The drawn height of an unresized ring mark, in world units.
///
/// The canvas paints the mark at this size times [`mark_weight`], so the room
/// reserved for it has to be computed from the same two numbers or the reserved
/// band and the drawn glyph disagree — which is exactly how a scaled-up circle's
/// sigil ended up under its container's wall.
pub const MARK_HEIGHT: f64 = 18.0;

/// The chip drawn behind a mark clears the wall by this much above the glyph.
const MARK_CHIP_PAD: f64 = 2.0;

/// How wide one character of a mark is, as a fraction of its height.
///
/// Marks are written in the monospace face, where every character advances the same
/// amount, so a mark's width is its length times this. The model cannot measure a
/// font — it has none — so this is the one place a rendering fact is written down
/// here, and it is deliberately an OVER-estimate: reserving a little too much room
/// leaves a gap, while reserving too little puts one circle's name through another's.
/// A test in the visual crate holds it against the real face.
pub const MARK_ADVANCE: f64 = 0.62;

/// How much larger a mark grows than the form it names, capped.
///
/// A sigil on a wide outer circle reads larger than one on a circle deep inside
/// it, which is how the eye is told which level it is looking at. The cap keeps
/// a heavily resized boundary from demanding unbounded room for one character.
const MARK_WEIGHT_CAP: f64 = 8.0;

/// How much a form's ring mark is magnified, given how far the form itself was
/// resized from its base size.
///
/// The fourth root of the area ratio, floored at 1 and capped: a mark grows with
/// its boundary, but far more slowly, so a circle scaled 400× wears a sigil 8×
/// rather than 400× the base size. The canvas reads this to size the glyph and
/// the layout reads it to reserve the room — one function so they cannot drift.
#[must_use]
pub fn mark_weight(resize: (f64, f64)) -> f64 {
    (resize.0 * resize.1)
        .sqrt()
        .max(1.0)
        .sqrt()
        .min(MARK_WEIGHT_CAP)
}

/// Room a form needs *outside* its own outline for the sigil written on its ring.
///
/// The mark is painted centred on the boundary's topmost point, so half of it
/// stands outside the outline and a container closing on the outline alone cuts
/// through it. `extent` is the form's resolved half-size, from which the mark's
/// magnification is recovered.
///
/// Room a boundary needs INSIDE its own ring for the mark written on it.
///
/// A mark is centred on the outline, so half of it hangs inside. Contents that reach
/// that far in are drawn straight through it — and with one content, which sits at the
/// middle, the boundary's radius exceeds its content's by only the content's own band
/// plus a pad, so the two marks ended up a few units apart and collided. Measured
/// across the standard library before this was reserved: thirty-odd overlapping pairs,
/// every one of them a boundary against something it holds.
fn mark_inward(node: &Node, prefix: &str, extent: (f64, f64)) -> f64 {
    // The prefix counts: a quoted `( )` wears `'` on its ring and nothing else, so a
    // boundary with no mark of its own may still have something written on it.
    if node.mark().is_empty() && prefix.is_empty() {
        return 0.0;
    }
    let base = default_extent(node);
    let resize = (
        extent.0 / base.0.max(f64::EPSILON),
        extent.1 / base.1.max(f64::EPSILON),
    );
    MARK_HEIGHT * mark_weight(resize) / 2.0 + MARK_CHIP_PAD
}

/// Every circle reserves at least [`LABEL_BAND`] whether it wears a mark today or
/// not, so a bare `( )` takes the room a `$` takes and a row of them reads as a
/// row. A form carrying a mark reserves that floor or the mark's own reach,
/// whichever is larger.
///
/// The reach counts the mark's WIDTH, not only its height. A mark is not always one
/// sigil: a call wears its callee's name, so `std-with-evidence` is written across
/// the top of its circle and reaches far further sideways than up. Reserving by
/// height alone gave a seventeen-character name the same room as a `$` — sixteen
/// units against the ninety it needed on each side — and neighbouring circles'
/// names ran through one another.
///
/// The mark is centred on the boundary's topmost point, so what has to be contained
/// is the far corner of its box: half the text's width across, and half its height
/// above the outline.
fn mark_band(node: &Node, prefix: &str, extent: (f64, f64)) -> f64 {
    let floor = if node.shape() == Shape::Circle {
        LABEL_BAND
    } else {
        0.0
    };
    let mark = node.mark();
    if mark.is_empty() && prefix.is_empty() {
        return floor;
    }
    let base = default_extent(node);
    let resize = (
        extent.0 / base.0.max(f64::EPSILON),
        extent.1 / base.1.max(f64::EPSILON),
    );
    let height = MARK_HEIGHT * mark_weight(resize);
    #[allow(clippy::cast_precision_loss)]
    let characters = (prefix.chars().count() + mark.chars().count()) as f64;
    let across = characters * height * MARK_ADVANCE / 2.0;
    let above = extent.1 + height / 2.0 + MARK_CHIP_PAD;
    // The radius that contains that corner, less the outline already drawn.
    let reach = across.hypot(above) - extent.0.max(extent.1);
    floor.max(reach)
}

fn default_extent(node: &Node) -> (f64, f64) {
    node.base_extent()
}

fn base_extent(node: &Node) -> (f64, f64) {
    let sized = sized_extent(node);
    match node.floor {
        Some((half_w, half_h)) => (sized.0.max(half_w), sized.1.max(half_h)),
        None => sized,
    }
}

fn sized_extent(node: &Node) -> (f64, f64) {
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
            floor: None,
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
    /// How many boundaries this form is drawn inside — its indentation on the
    /// canvas, which is its indentation in the source.
    ///
    /// Presentation nesting, not the AST: a mark written on a form and the form
    /// itself sit at the same level, because on the page they are one thing.
    #[must_use]
    pub fn indentation_depth(&self, id: NodeId) -> usize {
        let mut depth = 0usize;
        let mut cursor = id;
        let mut seen = HashSet::new();
        while seen.insert(cursor) {
            let Some(holder) = self.holder(cursor) else {
                break;
            };
            depth += 1;
            cursor = holder;
        }
        depth
    }

    fn layout_contents(&mut self, container: NodeId) {
        let Some(centre) = self.node(container).map(|node| (node.x, node.y)) else {
            return;
        };
        let contents = self
            .contained_children(container)
            .into_iter()
            .map(|id| (id, self.fit_extent(id)))
            .collect::<Vec<_>>();
        // Brothers are drawn at ONE size — the largest any of them needs.
        //
        // A row of siblings at assorted sizes reads as a hierarchy that is not
        // there. The forms inside one boundary are peers, and what tells them apart
        // is what they hold and what they are called, not how big they happened to
        // come out. So size stops carrying accidental information at a level, and
        // only nesting changes it.
        //
        // "One size" means among brothers of the same KIND: a boundary matches the
        // widest boundary beside it, a form holding nothing matches the widest of
        // those. Not one size across both, because a boundary's size is *derived*
        // from its contents — forcing a bare symbol up to the width of the loaded
        // circle next to it raises the parent's fit, which raises that parent's
        // brothers, and so on outward. Measured on the collection, equalising
        // across kinds cost 11× the page and 42× at depth nine, which is the same
        // compounding that killed an earlier attempt at this. Within a kind it
        // costs 1.7×.
        let widest = |mandala: &Self, holds: bool| {
            contents
                .iter()
                .filter(|(id, _)| mandala.contained_children(*id).is_empty() != holds)
                .fold((0.0_f64, 0.0_f64), |widest, (_, fit)| {
                    (widest.0.max(fit.0), widest.1.max(fit.1))
                })
        };
        let (boundaries, marks) = (widest(self, true), widest(self, false));
        // Under `Sizing::Even` the level is not the unit — the drawing is. Every
        // form that holds nothing is drawn at one size wherever it stands, and
        // every boundary is exactly as large as what it holds, so nothing is
        // sized by how deep it happens to be written. Nothing is filled to a
        // wall either: filling is what makes two forms at the same depth come
        // out at different sizes, which is the thing being evened out.
        let even = self.sizing == Sizing::Even;
        let everywhere = if even {
            self.even_mark_size()
        } else {
            (0.0, 0.0)
        };
        for (id, fit) in &contents {
            let holds = !self.contained_children(*id).is_empty();
            let size = match (even, holds) {
                (true, true) => *fit,
                (true, false) => everywhere,
                (false, true) => boundaries,
                (false, false) => marks,
            };
            // One level is one size, so a boundary needing less than its widest
            // brother is handed a wall it does not fill. The slack goes to what
            // it holds — see [`Self::fill_boundary`] — before the wall is set,
            // so the wall then closes on contents that already reach it.
            if !even {
                self.fill_boundary(*id, size.0, size.1);
            }
            self.resize(*id, size.0, size.1);
        }
        // Pack with the sizes they will actually be drawn at.
        // Packed by footprint, so a neighbour is never set against the wall a
        // ring label is written on.
        let items = contents
            .iter()
            .map(|(id, _)| {
                let footprint = self.footprint(*id);
                let shape = self.node(*id).map_or(Shape::Circle, |node| node.shape());
                (*id, footprint, circumscribed_reach(shape, footprint))
            })
            .collect::<Vec<_>>();
        // The figure follows the boundary: a box lays its contents out on the golden
        // section, a round one winds them along the spiral. Each is the arrangement
        // its own outline implies — rows and columns inside something with sides, a
        // curve inside something without.
        let boxed = self
            .node(container)
            .is_some_and(|node| node.shape() == Shape::Square);
        let spots = if boxed {
            golden_grid_spots(&items)
        } else {
            spiral_spots(&items)
        };
        for (id, x, y) in spots {
            self.move_group_to(id, centre.0 + x, centre.1 + y);
        }
        // Prefix sigils ride on the form they are written on, so they never
        // linger where a previous arrangement left them.
        let riders = self
            .nodes
            .iter()
            .map(|node| node.id)
            .filter(|id| self.is_written_prefix(*id))
            .filter_map(|id| {
                let operand = *self.children(id).first()?;
                let node = self.node(operand)?;
                Some((id, node.x, node.y))
            })
            .collect::<Vec<_>>();
        for (id, x, y) in riders {
            self.move_to(id, x, y);
        }
    }

    /// The prefix sigils written on the front of this form, outermost first.
    ///
    /// `',x` is one written thing: a quote of an unquote of `x`. The sigils are
    /// not forms standing beside `x`, they are marks on it, so the drawing puts
    /// them there — `',` in front of the symbol — instead of leaving three
    /// separate shapes for the eye to reassemble.
    #[must_use]
    pub fn written_prefix(&self, id: NodeId) -> String {
        let mut sigils = Vec::new();
        let mut cursor = id;
        let mut seen = HashSet::new();
        while seen.insert(cursor) {
            let Some(father) = self.father(cursor) else {
                break;
            };
            let Some(node) = self.node(father) else {
                break;
            };
            if !node.form.is_prefix_sigil() {
                break;
            }
            sigils.push(node.form.prefix_sigil());
            cursor = father;
        }
        sigils.reverse();
        sigils.concat()
    }

    /// The form actually drawn for this one.
    ///
    /// A prefix sigil is painted on the front of its operand rather than as a
    /// shape of its own, so it is never what anything on the canvas attaches
    /// to — the form it marks is. Following the chain answers "what is at this
    /// position", which is what a connection needs to know before it can point
    /// at something.
    #[must_use]
    pub fn drawn_form(&self, id: NodeId) -> NodeId {
        let mut cursor = id;
        let mut seen = HashSet::new();
        while seen.insert(cursor) {
            if !self.is_written_prefix(cursor) {
                break;
            }
            match self.children(cursor).first().copied() {
                Some(operand) => cursor = operand,
                None => break,
            }
        }
        cursor
    }

    /// The outermost flow whose chain this form takes part in, if any.
    ///
    /// `(-> a b c)` folds into nested flows, so the three stages are spread over two
    /// `->` nodes. A reader sees one chain, and so should a click: the whole run of
    /// arrows is one thing, and picking up its middle should pick up the rest.
    #[must_use]
    pub fn flow_chain_root(&self, id: NodeId) -> Option<NodeId> {
        let mut cursor = self
            .node(id)
            .filter(|node| node.form.is_flow())
            .map(|_| id)
            .or_else(|| self.father(id).filter(|f| self.is_flow_node(*f)))?;
        let mut seen = HashSet::new();
        while seen.insert(cursor) {
            let Some(father) = self.father(cursor) else {
                break;
            };
            if !self.is_flow_node(father) {
                break;
            }
            cursor = father;
        }
        Some(cursor)
    }

    fn is_flow_node(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|node| node.form.is_flow())
    }

    /// Every stage of one chain, in written order.
    ///
    /// The stages are what the arrows run BETWEEN — the flows themselves are the
    /// arrows and are not stages. `(-> (-> a b) c)` and `(-> a (-> b c))` both give
    /// `[a, b, c]`, because both draw the same three blocks joined by two arrows.
    #[must_use]
    pub fn flow_stages(&self, root: NodeId) -> Vec<NodeId> {
        let mut stages = Vec::new();
        let mut seen = HashSet::new();
        self.walk_chain(root, &mut stages, &mut seen);
        stages
    }

    fn walk_chain(&self, node: NodeId, stages: &mut Vec<NodeId>, seen: &mut HashSet<NodeId>) {
        if !seen.insert(node) {
            return;
        }
        for kid in self.children(node) {
            if self.is_flow_node(kid) {
                self.walk_chain(kid, stages, seen);
            } else {
                stages.push(self.drawn_form(kid));
            }
        }
    }

    /// Which stage of its chain this form is, one-based, and how many there are.
    #[must_use]
    pub fn flow_stage_number(&self, id: NodeId) -> Option<(usize, usize)> {
        let root = self.flow_chain_root(id)?;
        let stages = self.flow_stages(root);
        let at = stages.iter().position(|stage| *stage == id)?;
        Some((at + 1, stages.len()))
    }

    /// Move one stage of a chain to a new position, one-based.
    ///
    /// The stages are permuted where they SIT — the arrows keep their shape and the
    /// forms exchange slots — so `(-> a b c)` with `c` moved to 1 becomes
    /// `(-> c a b)`. The same mechanism as [`Self::set_child_number`], over a
    /// chain's slots instead of one father's.
    pub fn set_flow_stage_number(&mut self, id: NodeId, number: usize) -> bool {
        let Some(root) = self.flow_chain_root(id) else {
            return false;
        };
        let slots = self.chain_slots(root);
        if number == 0 || number > slots.len() {
            return false;
        }
        let occupants = slots
            .iter()
            .map(|slot| self.arrows[*slot].from)
            .collect::<Vec<_>>();
        let Some(current) = occupants.iter().position(|node| *node == id) else {
            return false;
        };
        let target = number - 1;
        if current == target {
            return false;
        }
        let mut ordered = occupants;
        let moved = ordered.remove(current);
        ordered.insert(target, moved);
        for (slot, occupant) in slots.into_iter().zip(ordered) {
            self.arrows[slot].from = occupant;
        }
        self.invalidate_geometry();
        true
    }

    /// The arrow slots holding a chain's stages, in written order.
    fn chain_slots(&self, root: NodeId) -> Vec<usize> {
        let mut slots = Vec::new();
        let mut seen = HashSet::new();
        self.walk_chain_slots(root, &mut slots, &mut seen);
        slots
    }

    fn walk_chain_slots(&self, node: NodeId, slots: &mut Vec<usize>, seen: &mut HashSet<NodeId>) {
        if !seen.insert(node) {
            return;
        }
        for kid in self.children(node) {
            if self.is_flow_node(kid) {
                self.walk_chain_slots(kid, slots, seen);
            } else if let Some(slot) = self
                .arrows
                .iter()
                .position(|arrow| arrow.from == kid && arrow.to == node)
            {
                slots.push(slot);
            }
        }
    }

    /// The form an arrow attaches to at one end of a connection.
    ///
    /// Two things resolve away. A prefix sigil is painted on the front of what it
    /// marks, so pointing at the sigil pointed at nothing. And a flow is drawn as
    /// an arrow rather than as a shape, so an arrow reaching a nested flow has to
    /// reach the STAGE of it that carries the value at that end: a forward flow
    /// ends at its second operand and begins at its first, and a backflow is the
    /// same relation read the other way.
    ///
    /// That makes chains draw as chains. `(-> (-> a b) c)` is `a → b → c`, because
    /// the outer arrow leaves the inner flow's value — which is `b` — rather than
    /// leaving a shape that is not there. Without this an arrow arrived from empty
    /// canvas with a head on the end of it.
    ///
    /// `producing` asks for the end that supplies a value; `false` asks for the end
    /// that receives one.
    #[must_use]
    pub fn flow_endpoint(&self, id: NodeId, producing: bool) -> NodeId {
        let mut cursor = id;
        let mut seen = HashSet::new();
        while seen.insert(cursor) {
            if self.is_written_prefix(cursor) {
                match self.children(cursor).first().copied() {
                    Some(operand) => cursor = operand,
                    None => break,
                }
                continue;
            }
            let Some(node) = self.node(cursor) else { break };
            if !node.form.is_flow() {
                break;
            }
            let kids = self.children(cursor);
            let [first, second] = kids[..] else {
                // An incomplete flow is still a small selectable arrow of its own.
                break;
            };
            let forward = node.form == Form::Forward;
            cursor = if producing == forward { second } else { first };
        }
        cursor
    }

    /// Whether this form is a prefix sigil, and so is drawn on its operand
    /// rather than anywhere of its own.
    #[must_use]
    pub fn is_written_prefix(&self, id: NodeId) -> bool {
        self.node(id)
            .is_some_and(|node| node.form.is_prefix_sigil() && self.children(id).len() == 1)
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
        if !(form.opens_indentation() || form.is_prefix_sigil()) || !self.has(id) {
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
            // A prefix claims no slot of its own; its operand keeps the one it
            // already has and the mark rides along on it.
            if self.is_written_prefix(made) {
                self.hold(holder, id);
            } else {
                self.hold(holder, made);
                self.hold(made, id);
            }
        } else if !self.is_written_prefix(made) {
            self.hold(made, id);
        }
        Some(made)
    }

    /// The prefix sigil written directly on this form, if it is that one.
    #[must_use]
    pub fn prefixed_with(&self, id: NodeId, form: &Form) -> Option<NodeId> {
        let father = self.father(id)?;
        let node = self.node(father)?;
        (node.form == *form && self.is_written_prefix(father)).then_some(father)
    }

    /// Take one form out of the drawing and let what it held take its place.
    ///
    /// The counterpart of [`Self::wrap`]. Deleting a boundary leaves everything
    /// inside it orphaned — no parent, no boundary, and source that no longer
    /// parses — so removing a level of nesting had no move at all. Here the
    /// operands are spliced into the hole: they inherit the form's place in its
    /// father's operand order and the boundary that was drawing it.
    ///
    /// Returns the forms that moved up.
    pub fn unwrap_form(&mut self, id: NodeId) -> Vec<NodeId> {
        let Some(operands) = self.has(id).then(|| self.children(id)) else {
            return Vec::new();
        };
        let father = self.father(id);
        let number = father.and_then(|father| self.child_number(father, id));
        // A prefix never held its operand — the operand kept the slot and the
        // mark rode on it — so taking the mark off must leave that slot alone.
        let holder = (!self.is_written_prefix(id)).then(|| self.holder(id));
        for (offset, operand) in operands.iter().copied().enumerate() {
            match father {
                Some(father) => {
                    self.reparent(father, operand);
                    if let Some(number) = number {
                        self.set_child_number(father, operand, number + offset);
                    }
                }
                None => {
                    self.detach(operand);
                }
            }
            match holder {
                Some(Some(holder)) => {
                    self.hold(holder, operand);
                }
                Some(None) => {
                    self.release(operand);
                }
                None => {}
            }
        }
        self.remove(id);
        operands
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
            // A block cut out of a drawing is still read the way that drawing
            // was read.
            legend: self.legend,
            sizing: self.sizing,
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
        // inner form must select the form, not its surrounding boundary. A
        // prefix sigil is drawn on the front of its operand rather than as a
        // shape of its own, so it is never what the pointer is on — the form it
        // marks is, and it sits at the same place.
        let inside = self.nodes.iter().rev().find_map(|n| {
            (!self.is_written_prefix(n.id) && self.is_inlined(n.id) && self.node_contains(n, x, y))
                .then_some(n.id)
        });
        if inside.is_some() {
            return inside;
        }
        let shape = self.nodes.iter().rev().find_map(|n| {
            (!self.is_written_prefix(n.id) && self.node_contains(n, x, y)).then_some(n.id)
        });
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
        // A nested boundary is a new surface. Paint the outer surface before
        // the inner one regardless of insertion order, so the inner circle or
        // brace can fully occlude the parent's edge where the two overlap.
        // Without this, dragging a boundary into another made their outlines
        // interpolate through one another simply because the later-created
        // node happened to be painted last.
        let mut containers = containers;
        containers.sort_by_key(|id| self.containment_depth(*id));
        containers.into_iter().chain(forms).collect()
    }

    /// Number of visual container walls between a node and the open canvas.
    /// Cycles are rejected by [`Self::hold`], but the visited set keeps this
    /// presentation ordering defensive when loading older documents.
    fn containment_depth(&self, id: NodeId) -> usize {
        let mut depth = 0;
        let mut cursor = id;
        let mut seen = HashSet::new();
        while seen.insert(cursor) {
            let Some(parent) = self.holder(cursor) else {
                break;
            };
            depth += 1;
            cursor = parent;
        }
        depth
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
        // Every boundary this form is drawn inside keeps the size it already had.
        //
        // A boundary's size is DERIVED from what it holds, so making one form
        // smaller pulled its container in after it — and the container's container,
        // and so on out to the page. Shrinking one circle rearranged the whole
        // drawing. Growing is different and has to stay: a wall must keep containing
        // what it holds, or the form escapes it.
        //
        // So the walls are pinned here, which only sets a FLOOR: the drawn size is
        // still the larger of the pin and what the contents need, so the container
        // grows when it must and stands still otherwise. `format mandala` is the
        // gesture that hands a boundary back to its contents.
        self.pin_enclosing(id);
        let factor = self.group_scale(id, half_w, half_h);
        self.scale_interior(id, factor);
        self.resize(id, half_w, half_h);
    }

    /// Scale everything drawn inside a boundary about that boundary's centre.
    ///
    /// Positions and extents take the same one factor, so the interior keeps its
    /// arrangement exactly: whatever cleared its neighbour before still clears
    /// it, at every size.
    fn scale_interior(&mut self, id: NodeId, factor: f64) {
        let Some(centre) = self
            .node(id)
            .map(|node| (node.x, node.y))
            .filter(|_| factor.is_finite() && (factor - 1.0).abs() > f64::EPSILON)
        else {
            return;
        };
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

    /// The one size every form that holds nothing is drawn at under
    /// [`Sizing::Even`]: the largest any of them needs.
    ///
    /// Taken across the whole drawing rather than a level, which is the entire
    /// difference between the two sizings. It is the largest and not an average
    /// because a size is a floor — a form drawn smaller than its own text or
    /// its own outline is not evened out, it is broken.
    fn even_mark_size(&self) -> (f64, f64) {
        self.nodes
            .iter()
            .map(|node| node.id)
            .filter(|id| self.contained_children(*id).is_empty())
            .map(|id| self.fit_extent(id))
            .fold((NODE_R, NODE_RY), |widest, fit| {
                (widest.0.max(fit.0), widest.1.max(fit.1))
            })
    }

    /// Grow what a boundary holds until it reaches the wall it is about to be
    /// given.
    ///
    /// A level is drawn at one size, so a boundary that needs less than its
    /// widest brother is handed the same wall as the rest — and what stands
    /// inside it used to stay whatever size it happened to be, a speck in an
    /// empty room. The room said nothing except which brother was the largest.
    /// Here the interior takes that slack instead, at one factor, so a circle
    /// holding a single prompt reads as that prompt rather than as a hole.
    ///
    /// Iterated because a boundary's fit is its contents *plus a constant* — the
    /// pad and the ring band do not scale with what is inside — so one factor
    /// falls a little short. Each pass removes the same fraction of what is
    /// left, and a handful settle far inside a pixel. Stopping early when the
    /// floor rather than the wall decides the factor keeps a contents-limited
    /// boundary from being scaled again on every pass.
    fn fill_boundary(&mut self, id: NodeId, half_w: f64, half_h: f64) {
        if self.interior(id).is_empty() {
            return;
        }
        for _ in 0..FILL_PASSES {
            let fit = self.fit_extent(id);
            if fit.0 <= 0.0 || fit.1 <= 0.0 {
                return;
            }
            // One factor from the tighter axis: the interior is scaled, never
            // stretched, whatever shape of wall it is being fitted to.
            let wanted = (half_w / fit.0).min(half_h / fit.1);
            let factor = wanted.max(self.smallest_group_scale(id));
            if !factor.is_finite() || (factor - 1.0).abs() <= FILL_TOLERANCE {
                return;
            }
            self.scale_interior(id, factor);
            if factor > wanted {
                return;
            }
        }
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

    /// The room this form needs on the canvas, label included.
    ///
    /// [`Self::extent`] is the outline: what is drawn, what is clicked, what a
    /// wall is dragged by. This is that plus the band a ring label occupies, and
    /// it is what anything *arranging* forms must read — packing them apart, the
    /// boundary closing around them, the framing that fits a drawing to a
    /// window.
    ///
    /// The band is sized from the mark this form actually draws, so it grows with
    /// a resized boundary and is nothing at all for a form that wears no sigil —
    /// a bare `( )` states itself with its circle and takes no extra room.
    #[must_use]
    pub fn footprint(&self, id: NodeId) -> (f64, f64) {
        let extent = self.extent(id);
        match self.node(id) {
            Some(node) => {
                let band = mark_band(node, &self.written_prefix(id), extent);
                (extent.0 + band, extent.1 + band)
            }
            None => extent,
        }
    }

    /// The complete drawn bounds of everything on the canvas, extents included.
    ///
    /// `None` for an empty drawing, which has no bounds to speak of rather than
    /// a zero-sized one at the origin.
    #[must_use]
    pub fn bounds(&self) -> Option<WorldRect> {
        let mut bounds: Option<WorldRect> = None;
        for node in &self.nodes {
            let extent = self.footprint(node.id);
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

    /// Hold every boundary this form is drawn inside at the size it has now.
    ///
    /// A floor, not a fixed size — see [`Self::resize_group`] for why a hand
    /// gesture may grow a container but not shrink one. Read before written, so an
    /// outer wall is pinned at what it measured before any inner one moved.
    fn pin_enclosing(&mut self, id: NodeId) {
        let mut pins = Vec::new();
        let mut cursor = id;
        let mut seen = HashSet::new();
        while seen.insert(cursor) {
            let Some(holder) = self.holder(cursor) else {
                break;
            };
            pins.push((holder, self.extent(holder)));
            cursor = holder;
        }
        for (holder, extent) in pins {
            if let Some(node) = self.nodes.iter_mut().find(|node| node.id == holder) {
                node.floor = Some(extent);
            }
        }
        self.invalidate_geometry();
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

    /// How much of each form's text the drawing writes inside it.
    #[must_use]
    pub fn legend(&self) -> Legend {
        self.legend
    }

    /// Write one token per form, or all of the text. Returns whether anything
    /// changed, so a caller can skip the relayout that a change wants.
    ///
    /// Outlines resize themselves the moment this changes — the room a caption
    /// needs is derived, not stored — but nothing MOVES on its own. Growing a
    /// form where it stands would push it through its neighbour, so the caller
    /// relayouts to repack at the new sizes.
    pub fn set_legend(&mut self, legend: Legend) -> bool {
        if self.legend == legend {
            return false;
        }
        self.legend = legend;
        self.invalidate_geometry();
        true
    }

    /// How sizes relate across the drawing.
    #[must_use]
    pub fn sizing(&self) -> Sizing {
        self.sizing
    }

    /// Choose between per-level sizes and one size for the whole drawing.
    /// Returns whether anything changed. Takes effect on the next layout.
    pub fn set_sizing(&mut self, sizing: Sizing) -> bool {
        if self.sizing == sizing {
            return false;
        }
        self.sizing = sizing;
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

    /// Generate Rebis for everything on the page, however many top-level forms
    /// it has.
    ///
    /// [`Self::to_rebis`] answers "which one expression is this?" — what a
    /// block, a run, and a round trip each need, and it refuses a drawing with
    /// two roots because two answers are not an answer. This answers "what is
    /// written here?", and a page of Rebis is *allowed* to be several forms:
    /// the language's own top level is a list of them, which is why a file can
    /// open with imports and macro definitions before the expression that uses
    /// them.
    ///
    /// It exists because the strict reading made the source panel go quiet
    /// exactly when it was most wanted. Every drawing passes through several
    /// roots on its way to having one — a form placed is a root until it is
    /// linked — so the panel showed nothing for the whole of the gesture that
    /// was creating the program.
    ///
    /// Roots are written in the order they were drawn. Everything that is a
    /// real error under the single-expression reading — a cycle, a shared
    /// subexpression, the wrong arity — is still an error here.
    ///
    /// # Errors
    ///
    /// Returns the same [`MandalaError`]s as [`Self::to_rebis`], except that
    /// several roots are not one of them.
    pub fn to_rebis_page(&self) -> Result<String, MandalaError> {
        if self.nodes.is_empty() {
            return Err(MandalaError::Empty);
        }
        let roots: Vec<NodeId> = self
            .nodes
            .iter()
            .map(|node| node.id)
            .filter(|id| !self.arrows.iter().any(|arrow| arrow.from == *id))
            .collect();
        if roots.is_empty() {
            return Err(MandalaError::NoRoot);
        }
        // One `seen` across the whole page: it is what proves every node was
        // reached exactly once, and a node reachable from two roots is the
        // shared-subexpression error rather than a form written twice.
        let mut seen = HashSet::new();
        let mut forms = Vec::with_capacity(roots.len());
        for root in &roots {
            let mut on_path = HashSet::new();
            forms.push(self.render(*root, true, false, &mut on_path, &mut seen)?);
        }
        if seen.len() != self.nodes.len() {
            return Err(MandalaError::Cycle);
        }
        let source = forms.join("\n");
        let expression = rebis_lang::parse(&source)
            .map_err(|error| MandalaError::InvalidSource(error.to_string()))?;
        let structural = match (&expression, roots.as_slice()) {
            (rebis_lang::Expr::Program(items), roots) if items.len() == roots.len() => roots
                .iter()
                .zip(items)
                .all(|(id, item)| self.matches_expression(*id, item)),
            (expression, [only]) => self.matches_expression(*only, expression),
            _ => false,
        };
        if !structural {
            return Err(MandalaError::InvalidSource(
                "a source payload changed the expression structure".to_string(),
            ));
        }
        Ok(source)
    }

    /// Generate Rebis source for this mandala.
    pub fn to_rebis(&self) -> Result<String, MandalaError> {
        let root = self.root()?;
        let mut on_path = HashSet::new();
        let mut seen = HashSet::new();
        let source = self.render(root, true, false, &mut on_path, &mut seen)?;
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
        // A `(/ …)` in the source is either a Route node — a written route,
        // matched by the arm below — or the wrapper a per-form pin generated
        // around some other form, which is unwrapped here so the form beneath
        // is what gets matched. Which one it is, is what the node says.
        let (expression, model) = match expression {
            Expr::Model { selector, body } if node.form != Form::Route => {
                (body.as_ref(), Some(selector.as_str()))
            }
            expression => (expression, None),
        };
        if node.form != Form::Route && node.model.as_deref() != model {
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
            | (Form::Dream, Expr::Dream(inner))
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
            // A two-child head-and-scope: the head is inert and the scope is
            // what it holds, so both must match in order.
            (Form::Supersede, Expr::Supersede { topic: head, body })
            | (Form::Invariant, Expr::Invariant { check: head, body }) => {
                children.len() == 2 && child_matches(0, head) && child_matches(1, body)
            }
            // `=` carries its name on the node and its two scopes as children.
            // It had no arm at all until a numeric round-trip went looking for
            // one, so `(= n A B)` has never survived being drawn and read back.
            (Form::Bind, Expr::Bind { name, value, body }) => {
                node.text.trim() == name
                    && children.len() == 2
                    && child_matches(0, value)
                    && child_matches(1, body)
            }
            // An atom that names the program. Nothing to compare but itself.
            (Form::Source, Expr::Source) => children.is_empty(),
            (Form::Concat, Expr::Concat(items))
            | (Form::Flashback, Expr::Flashback(items))
            | (Form::Compose, Expr::Compose(items))
            | (Form::Imaginary, Expr::Imaginary(items))
            | (Form::Numeric, Expr::Numeric(items))
            | (Form::Meta, Expr::Meta(items))
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
            (Form::Input, Expr::Ask) => children.is_empty(),
            (Form::Load, Expr::Load(path)) => child_matches(0, path),
            (Form::Route, Expr::Model { selector, body }) => {
                node.text == selector.to_string() && child_matches(0, body)
            }
            (Form::Context, Expr::Context { context, body }) => {
                children.len() == 2 && child_matches(0, context) && child_matches(1, body)
            }
            _ => false,
        }
    }

    fn render(
        &self,
        id: NodeId,
        at_root: bool,
        quoted: bool,
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

        // An empty `Compose` is the EMPTY LIST, and it is legal in exactly one
        // place: directly under a quote. That is the language's own rule —
        // `'()` parses, a bare `()` does not — and the position decides the
        // reading here for the same reason it decides it there. Checked with
        // the parent in hand rather than by widening `Compose`'s arity, which
        // would have let the canvas build an `()` that no parser accepts.
        let empty_list = quoted && matches!(node.form, Form::Compose) && kids.is_empty();
        let arity = node.form.arity();
        if !empty_list && !arity.accepts(kids.len()) {
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
            parts.push(self.render(kid, false, matches!(node.form, Form::Quote), on_path, seen)?);
        }
        on_path.remove(&id);

        let text = &node.text;
        let out = match &node.form {
            Form::Prompt => quote(text),
            Form::Symbol => text.clone(),
            Form::Source => "<>".to_string(),
            Form::Import => format!("(# {text})"),
            Form::Quote => format!("'{}", parts[0]),
            Form::Unquote => format!(",{}", parts[0]),
            Form::Invert => format!("(^ {})", parts[0]),
            Form::Forward => format!("(-> {} {})", parts[0], parts[1]),
            Form::Backflow => format!("(<- {} {})", parts[0], parts[1]),
            Form::Square => format!("([{}] {})", parts[0], parts[1..].join(" ")),
            Form::Conditional => format!("(% {} {} {})", parts[0], parts[1], parts[2]),
            Form::Concat => format!("($ {})", parts.join(" ")),
            Form::Flashback => format!("(? {})", parts.join(" ")),
            Form::Dream => format!("(! {})", parts.join(" ")),
            Form::Load => format!("(&: {})", parts.join(" ")),
            Form::Context => format!("(+ {})", parts.join(" ")),
            Form::Route => format!(
                "(/ {} {})",
                self.node(id).map_or("", |node| node.text.trim()),
                parts.join(" ")
            ),
            Form::Bind => format!(
                "(= {} {})",
                self.node(id).map_or("", |node| node.text.trim()),
                parts.join(" ")
            ),
            Form::Meta => format!("(>< {})", parts.join(" ")),
            Form::Numeric => format!("|{}|", parts.join(" ")),
            Form::Supersede => format!("(* {})", parts.join(" ")),
            Form::Invariant => format!("(@ {})", parts.join(" ")),
            Form::Compose => format!("({})", parts.join(" ")),
            Form::Imaginary => format!("{{{}}}", parts.join(" ")),
            Form::Call => format!("({text} {})", parts.join(" ")).replace(" )", ")"),
            Form::Function(params) => {
                format!("(~ {text} ({}) {})", params.join(" "), parts[0])
            }
            Form::Input => format!("(& {text} {})", parts[0]),
            Form::Program => parts.join("\n"),
        };
        Ok(match &node.model {
            // A per-form pin writes the routing form around the one form it
            // pins. It used to write a postfix suffix, which the language no
            // longer has: routing is a scope now, and a pin is the smallest
            // possible one.
            Some(model) => format!("(/ {model} {out})"),
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
/// spiral. The outermost level winds from here, exactly as a boundary's interior
/// winds from its own middle.
const CIRCUIT_ORIGIN: (f64, f64) = (150.0, 130.0);
/// Vertical gap between rows (component pitch).
const ROW_GAP: f64 = 96.0;
/// Clear canvas left between the resized bounds of vertically packed forms.
const ROW_GUTTER: f64 = ROW_GAP - NODE_R * 2.0;
/// The golden ratio, the one proportion the drawing is built on: the aspect a box's
/// contents are gridded to, and the step a ring mark grows by as the form it names
/// is enlarged.
const PHI: f64 = 1.618_033_988_749_895;

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

/// The radius of the smallest circle that contains this form.
///
/// What the packer clears around each form. A box has to count its CORNER, so a
/// square reaches its half-diagonal; every other shape on the canvas is inscribed
/// in its own bounds, so the wider half already contains it.
///
/// The distinction is worth a rule of its own because assuming corners everywhere
/// costs √2 on the common case: measured, clearing round forms as though they were
/// boxes left every diagonal neighbour 1.4× further away than it needed to be.
fn circumscribed_reach(shape: Shape, extent: (f64, f64)) -> f64 {
    if shape == Shape::Square {
        extent.0.hypot(extent.1)
    } else {
        extent.0.max(extent.1)
    }
}

/// Keeps equality on the non-overlapping side despite floating-point rounding in
/// the geometry cache that measures these positions afterwards.
const SPIRAL_SLACK: f64 = 1.0 + 1e-9;

/// Where the arm sits, and how far apart its turns are.
///
/// The curve is `r = b·θ` — Archimedean, the plain one: every turn sits the same
/// distance outside the turn within it. A logarithmic spiral was the other
/// candidate and it is the more beautiful figure, but its radius multiplies where
/// this one adds, so a boundary holding eight forms spanned φ⁷ ≈ 29 times its
/// innermost radius and the page grew past reading.
///
/// `b` is chosen from the widest form a boundary holds, so consecutive turns are
/// exactly far enough apart for the widest pair to clear. That makes every
/// cross-turn pair safe by construction — two forms a whole turn apart differ in
/// radius by `2πb` or more — and leaves only neighbours ALONG the arm to place.
fn spiral_turn(widest_reach: f64) -> f64 {
    (2.0 * widest_reach + CONTENT_GAP) * SPIRAL_SLACK / std::f64::consts::TAU
}

/// The point at angle `angle` on the spiral of turn spacing `b`.
fn spiral_at(b: f64, angle: f64) -> (f64, f64) {
    (b * angle * angle.cos(), b * angle * angle.sin())
}

/// The first angle past `from` at which the spiral stands `clearance` away from
/// `anchor`.
///
/// Forms advance along the arm by what each PAIR needs rather than by a constant,
/// which is what "shrunk together according to their size" means: a small form
/// takes a small step and a large one takes a large step, so no form is handed room
/// another form's size.
///
/// By chord, not by arc length. Near the middle the arm curves hard, so an arc of
/// 80 units spans a chord of only 45 — placing by arc length there put the first two
/// forms squarely on top of each other. The chord is the distance that has to clear,
/// so the chord is what is solved for. Distance from a fixed inner point grows over
/// the turn ahead, so a bisection finds the first crossing exactly.
fn spiral_advance(b: f64, from: f64, anchor: (f64, f64), clearance: f64) -> f64 {
    let mut low = from;
    let mut high = from + std::f64::consts::TAU;
    for _ in 0..60 {
        let mid = f64::midpoint(low, high);
        let at = spiral_at(b, mid);
        if (at.0 - anchor.0).hypot(at.1 - anchor.1) < clearance {
            low = mid;
        } else {
            high = mid;
        }
    }
    high
}

/// The golden-section positions for one BOX's direct contents.
///
/// A box is not a circle and should not be packed like one. Where a circle gets the
/// spiral — a figure with a centre and no sides — a box gets the figure its own
/// notation implies: rows and columns cut so the whole comes out a golden rectangle,
/// φ wide for every 1 tall.
///
/// The column count is chosen, not assumed. Given `n` forms in cells of one size,
/// each candidate width yields an aspect ratio, and the one landing nearest φ wins;
/// ties go to the wider arrangement, because a mediator reads left-to-right and a
/// tall stack of branches does not. So three branches lay out in a row, four in
/// 3×2 or 2×2 depending on the cell, and a dozen in the grid closest to the golden
/// rectangle rather than in whatever the spiral happened to give.
///
/// Tighter than the spiral, besides: a grid of equal cells wastes nothing between
/// them, where a spiral must leave the gaps its curve implies.
fn golden_grid_spots(items: &[(NodeId, (f64, f64), f64)]) -> Vec<(NodeId, f64, f64)> {
    if items.is_empty() {
        return Vec::new();
    }
    // One cell size for the whole grid — the widest form decides it, so the rows and
    // columns line up and the figure reads as a grid rather than as a drift.
    let cell = items
        .iter()
        .fold((0.0_f64, 0.0_f64), |cell, (_, extent, _)| {
            (cell.0.max(extent.0), cell.1.max(extent.1))
        });
    let (pitch_x, pitch_y) = (2.0 * cell.0 + CONTENT_GAP, 2.0 * cell.1 + CONTENT_GAP);
    #[allow(clippy::cast_precision_loss)]
    let count = items.len() as f64;
    let mut best = (1usize, f64::INFINITY);
    for columns in 1..=items.len() {
        #[allow(clippy::cast_precision_loss)]
        let wide = columns as f64;
        let rows = (count / wide).ceil();
        let aspect = (wide * pitch_x) / (rows * pitch_y);
        // Distance in log space, so φ wide and φ tall are equally wrong.
        let off = (aspect.ln() - PHI.ln()).abs();
        if off < best.1 - 1e-12 {
            best = (columns, off);
        }
    }
    let columns = best.0.max(1);
    let rows = items.len().div_ceil(columns);
    #[allow(clippy::cast_precision_loss)]
    let origin = (
        -(columns as f64 - 1.0) * pitch_x / 2.0,
        -(rows as f64 - 1.0) * pitch_y / 2.0,
    );
    items
        .iter()
        .enumerate()
        .map(|(index, (id, _, _))| {
            #[allow(clippy::cast_precision_loss)]
            let (column, row) = ((index % columns) as f64, (index / columns) as f64);
            (*id, origin.0 + column * pitch_x, origin.1 + row * pitch_y)
        })
        .collect()
}

/// The tightest non-overlapping spiral positions for one boundary's direct
/// contents.
///
/// Every form sits exactly on one Archimedean spiral, and each one advances along
/// it by what IT and its neighbour need — not by a constant step, and not by
/// opening the whole arm until the worst pair fits. That last one is what was here
/// before, and it is why a boundary holding two dozen small forms came out twelve
/// times an item's own radius when a tight packing needs seven: the arm was opened
/// until the tightest pair anywhere cleared, and every other form was handed the
/// same room whether it asked for it or not.
///
/// Advancing pairwise gives the density back. Cross-turn pairs are safe by
/// construction — see [`spiral_turn`] — so only neighbours along the arm are solved
/// for, and each is placed at the first angle where its own chord clears. A
/// correction pass then applies the smallest uniform opening that satisfies the
/// exact box test, which is a no-op unless a diagonal pair needs a hair more than
/// its circumscribed reach implied; a uniform opening keeps the figure the same
/// spiral, since scaling `r = bθ` gives `r = kbθ`.
fn spiral_spots(items: &[(NodeId, (f64, f64), f64)]) -> Vec<(NodeId, f64, f64)> {
    if items.is_empty() {
        return Vec::new();
    }
    let widest = items
        .iter()
        .map(|(_, _, reach)| *reach)
        .fold(0.0_f64, f64::max);
    let turn = spiral_turn(widest);
    let mut points = Vec::with_capacity(items.len());
    let mut angle = 0.0_f64;
    points.push(spiral_at(turn, angle));
    for pair in items.windows(2) {
        let clearance = (pair[0].2 + pair[1].2 + CONTENT_GAP) * SPIRAL_SLACK;
        let anchor = *points.last().expect("the first form is placed");
        angle = spiral_advance(turn, angle, anchor, clearance);
        points.push(spiral_at(turn, angle));
    }
    let mut spots = items
        .iter()
        .zip(&points)
        .map(|((id, extent, _), (x, y))| (*id, *x, *y, *extent))
        .collect::<Vec<_>>();
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
        // A route is a form now, so it is built like one. `Node::model` is
        // still set by the panel for a per-form pin; it is simply no longer how
        // a written `(/ …)` arrives.
        let model: Option<String> = None;
        let (form, text, kids): (Form, String, Vec<&Expr>) = match expr {
            Expr::Prompt(s) => (Form::Prompt, s.clone(), vec![]),
            Expr::Symbol(s) => (Form::Symbol, s.clone(), vec![]),
            Expr::Source => (Form::Source, String::new(), vec![]),
            Expr::Meta(v) => (Form::Meta, String::new(), v.iter().collect()),
            Expr::Numeric(v) => (Form::Numeric, String::new(), v.iter().collect()),
            Expr::Supersede { topic, body } => (Form::Supersede, String::new(), vec![topic, body]),
            Expr::Invariant { check, body } => (Form::Invariant, String::new(), vec![check, body]),
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
            Expr::Flashback(v) => (Form::Flashback, String::new(), v.iter().collect()),
            Expr::Dream(x) => (Form::Dream, String::new(), vec![x]),
            Expr::Bind { name, value, body } => (
                Form::Bind,
                name.clone(),
                vec![value.as_ref(), body.as_ref()],
            ),
            Expr::Compose(v) => (Form::Compose, String::new(), v.iter().collect()),
            Expr::Imaginary(v) => (Form::Imaginary, String::new(), v.iter().collect()),
            Expr::Call { name, args } => (Form::Call, name.clone(), args.iter().collect()),
            Expr::Function { name, params, body } => (
                Form::Function(params.clone()),
                name.clone(),
                vec![body.as_ref()],
            ),
            Expr::Ask => (Form::Input, String::new(), vec![]),
            Expr::Load(path) => (Form::Load, String::new(), vec![path.as_ref()]),
            Expr::Context { context, body } => (
                Form::Context,
                String::new(),
                vec![context.as_ref(), body.as_ref()],
            ),
            Expr::Program(v) => (Form::Program, String::new(), v.iter().collect()),
            Expr::Model { selector, body } => {
                (Form::Route, selector.to_string(), vec![body.as_ref()])
            }
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
            // A prefix sigil is drawn on the front of its operand, so it claims
            // no slot of its own; its operand takes the place it would have had.
            if self.is_written_prefix(id) {
                continue;
            }
            // A flow claims none either: it is the arrow drawn between the two
            // forms it routes, so those two take the places, side by side in the
            // boundary that encloses the flow. Giving the flow a slot of its own
            // drew a circle around the pair and made the connection look like a
            // container.
            if self.node(id).is_some_and(|node| node.form.is_flow()) {
                continue;
            }
            if self.holder(id).is_none() {
                let _ = self.hold(container, id);
            }
        }
    }

    /// Lay the syntax tree out, innermost first, everything on a spiral.
    ///
    /// Boundaries are packed from the inside out, so an outer arm sees the final
    /// size of every circle and square within it. Each boundary arranges its own
    /// contents along its own arm — a circle on a spiral, a box on the golden grid
    /// — and then the OUTERMOST level does the same, winding from
    /// [`CIRCUIT_ORIGIN`] as though the program itself were the boundary holding
    /// everything.
    ///
    /// That last part used to be a circuit: nesting depth chose the column and the
    /// rows were packed beneath it. It read badly for the commonest shape of
    /// program there is — a file of top-level definitions, all one deep, every one
    /// of them in the same column, the drawing a ribbon far taller than a screen.
    /// A spiral spends both directions.
    ///
    /// Coordinates and content placement are presentation only and never affect
    /// generated Rebis.
    fn layout(&mut self, depths: &[(NodeId, usize)]) {
        use std::cmp::Reverse;

        // Floors go first. A floor is a size a boundary was held at so a shrinking
        // content could not drag its wall in; formatting states what the drawing is
        // at the least size it can honestly be drawn at, so every wall is handed back
        // to its contents here. Hand-set sizes are NOT cleared — the circuit layout
        // reserves columns and rows for a symbol someone deliberately made larger.
        for node in &mut self.nodes {
            node.floor = None;
        }
        self.invalidate_geometry();

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

        // Under `Sizing::Even`, every form that holds nothing takes the one size
        // first — including the ones standing at the top level, which belong to
        // no boundary and so are never reached by the pass below. Sizing them
        // before anything is packed is also what lets each boundary close on
        // contents that are already final.
        if self.sizing == Sizing::Even {
            let size = self.even_mark_size();
            let marks = self
                .nodes
                .iter()
                .map(|node| node.id)
                .filter(|id| self.contained_children(*id).is_empty())
                // A size set by hand at the top level is a deliberate
                // statement about one form, and formatting has never undone
                // one. Inside a boundary the level rules, exactly as it does
                // under `ByLevel`.
                .filter(|id| self.is_inlined(*id) || self.hand_size(*id).is_none())
                .collect::<Vec<_>>();
            for id in marks {
                self.resize(id, size.0, size.1);
            }
        }

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
        containers.sort_by_key(|container| Reverse(self.indentation_depth(*container)));
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

        // The outermost level is a boundary like any other — the program itself —
        // so its forms wind along the same spiral its interiors do.
        //
        // They used to be laid out as a circuit: nesting depth chose the column and
        // the rows were packed beneath it. For a program that is mostly one deep —
        // a file of twenty-two top-level definitions, say — every one of them shares
        // a column, and the drawing comes out a ribbon eleven thousand units tall
        // that has to be read at six percent zoom. A spiral spends both directions.
        //
        // Source order along the arm, so the first definition is at the middle and
        // reading outward is reading down the file.
        let mut ordered = Vec::new();
        let mut placed = HashSet::new();
        for (id, _) in depths {
            if active.contains(id) && placed.insert(*id) {
                let footprint = self.footprint(*id);
                let shape = self.node(*id).map_or(Shape::Circle, |node| node.shape());
                ordered.push((*id, footprint, circumscribed_reach(shape, footprint)));
            }
        }
        for (id, x, y) in spiral_spots(&ordered) {
            let at = (CIRCUIT_ORIGIN.0 + x, CIRCUIT_ORIGIN.1 + y);
            if self
                .node(id)
                .is_some_and(|node| is_visual_container(&node.form))
            {
                self.move_group_to(id, at.0, at.1);
            } else {
                self.move_to(id, at.0, at.1);
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
        assert_round_trip("(? \"a topic\")"); // Flashback
        assert_round_trip("((\"local\") \"sub\")"); // Compose
        assert_round_trip("(f \"a\" \"b\")"); // Call
        assert_round_trip("\"p\" \"q\""); // Program
        assert_round_trip("(-> \"a\" \"b\")/claude:opus5"); // Model
    }

    #[test]
    fn a_flashback_is_drawn_and_written_like_the_indentation_it_is() {
        // `?` is parenthesised, so the canvas rule applies without exception: one
        // circle, wearing its own sigil, holding its topic as contents.
        for source in [
            "(? \"a topic\")",
            "(? retry queue)",
            "($ \"given \" (? topic) \" decide\")",
            "(-> (? topic) \"act on it\")",
            "(~ recall (topic) '(? ,topic))",
            "(? topic)/claude:opus5",
        ] {
            assert_round_trip(source);
        }

        let mandala = Mandala::from_rebis("(? retry queue)").expect("the fixture parses");
        let root = mandala.root().expect("the drawing has a root");
        let node = mandala.node(root).expect("the root exists");
        assert_eq!(node.form, Form::Flashback);
        assert_eq!(node.shape(), Shape::Circle, "an indentation is a circle");
        assert_eq!(node.mark(), "?", "the sigil rides the ring");
        assert_eq!(
            mandala.children(root).len(),
            2,
            "the topic words are its contents"
        );
    }

    #[test]
    fn routing_is_a_drawn_form_and_round_trips() {
        // Routing used to be a postfix suffix, unwrapped into a field, so a
        // drawing could not show which part of a program ran on which model —
        // the one consequential thing about a program that the canvas was
        // blind to. As a form it has a circle, and the routed subtree is drawn
        // inside it.
        let source = "(/ claude:opus5 \
                        (-> (/ ollama:qwen4:4b \"a\") \
                            (/ openrouter:anthropic/claude-opus-4 ([\"judge\"] \"b\" \"c\"))))";
        let mandala = Mandala::from_rebis(source).unwrap();

        let routes: Vec<&Node> = mandala
            .nodes()
            .iter()
            .filter(|node| node.form == Form::Route)
            .collect();
        assert_eq!(routes.len(), 3, "every route is a node");
        // Each wears its selector on its ring, which is what makes the routing
        // legible without opening a panel.
        let mut worn: Vec<String> = routes.iter().map(|node| node.mark()).collect();
        worn.sort();
        assert_eq!(
            worn,
            vec![
                "/ claude:opus5".to_string(),
                "/ ollama:qwen4:4b".to_string(),
                "/ openrouter:anthropic/claude-opus-4".to_string(),
            ]
        );
        // A route is an indentation like every other parenthesised form, and
        // holds exactly the one thing it routes.
        for route in &routes {
            assert!(route.form.opens_indentation());
            assert_eq!(route.shape(), Shape::Circle);
            assert_eq!(route.form.arity(), Arity::Exactly(1));
            assert_eq!(mandala.children(route.id).len(), 1);
        }

        let regenerated = mandala.to_rebis().unwrap();
        assert_eq!(
            rebis_lang::parse(&regenerated).unwrap(),
            rebis_lang::parse(source).unwrap()
        );
    }

    #[test]
    fn a_per_form_model_pin_is_still_metadata() {
        // The panel pins one form's model without wrapping it, and that stays
        // a field: it is an exception to a scope, not a scope of its own, and
        // giving it a circle would draw a boundary around a single form.
        let mut mandala = Mandala::new();
        let prompt = mandala.add(Form::Prompt, "draft", 0.0, 0.0);
        mandala.set_model(prompt, Some("ollama:qwen4:4b".to_string()));
        assert_eq!(mandala.nodes().len(), 1, "a pin added geometry");
        let regenerated = mandala.to_rebis().unwrap();
        assert_eq!(
            rebis_lang::parse(&regenerated).unwrap(),
            rebis_lang::parse("(/ ollama:qwen4:4b \"draft\")").unwrap()
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

        // An indentation shows the line that opened it — whole, because the
        // ring is where a boundary says which boundary it is.
        let macro_form = m.add(Form::Function(vec!["x".into()]), "twice", 0.0, 0.0);
        assert_eq!(m.node(macro_form).unwrap().glyph(), "~ twice (x)");

        // And a bare compose has nothing to say: its circle is the statement.
        let compose = m.add(Form::Compose, "", 0.0, 0.0);
        assert_eq!(m.node(compose).unwrap().glyph(), "");
    }

    #[test]
    fn every_parenthesised_form_is_a_circle_wearing_its_own_sigil() {
        // One rule: a form that writes its operands inside its own parentheses
        // is an indentation, an indentation is a circle, and the line that
        // opened it is written on the ring instead of loose among its contents.
        for (form, mark) in [
            (Form::Concat, "$"),
            (Form::Flashback, "?"),
            (Form::Invert, "^"),
            (Form::Conditional, "%"),
            (Form::Function(vec![]), "~ ()"),
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

        // And a form whose head has words in it wears the words. Two macros
        // side by side are two circles; without their names on the rings the
        // drawing cannot say which is which, and a caller cannot see what to
        // pass.
        let mut mandala = Mandala::new();
        let named = mandala.add(
            Form::Function(vec!["url".into(), "depth".into()]),
            "fetch",
            0.0,
            0.0,
        );
        assert_eq!(mandala.node(named).unwrap().mark(), "~ fetch (url depth)");
        let port = mandala.add(Form::Input, "review", 0.0, 0.0);
        assert_eq!(mandala.node(port).unwrap().mark(), "& review");
        let call = mandala.add(Form::Call, "fetch", 0.0, 0.0);
        assert_eq!(mandala.node(call).unwrap().mark(), "fetch");

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
    fn a_flow_is_the_arrow_and_claims_no_circle_of_its_own() {
        // A connection is not a container. `(-> A B)` draws A and B side by side in
        // whatever boundary encloses the flow, with the arrow between them — no
        // circle around the pair, because a circle says "these two are one thing"
        // when what is meant is "this one feeds that one".
        for flow in [Form::Forward, Form::Backflow] {
            assert!(flow.is_flow());
            assert!(!flow.opens_indentation(), "a flow is not an indentation");
            assert_eq!(flow.shape(), Shape::Arrow);
            let mut probe = Mandala::new();
            let id = probe.add(flow.clone(), "", 0.0, 0.0);
            assert_eq!(
                probe.node(id).unwrap().mark(),
                "",
                "a flow wears no title: the arrow is the whole of it"
            );
        }

        // Inside a boundary, the two operands are the boundary's own contents —
        // the flow takes no slot beside them.
        let mandala = Mandala::from_rebis("((-> \"a\" \"b\") \"c\")").unwrap();
        let compose = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose)
            .expect("the group")
            .id;
        let held = mandala.contained_children(compose);
        assert_eq!(
            held.len(),
            3,
            "two flow operands and the third form: {held:?}"
        );
        assert!(
            held.iter()
                .all(|id| mandala.node(*id).is_some_and(|node| !node.form.is_flow())),
            "a flow claimed a slot of its own"
        );
        assert_eq!(mandala.to_rebis().unwrap(), "((-> \"a\" \"b\") \"c\")");
    }

    #[test]
    fn a_chain_is_one_ordered_thing_however_it_was_written() {
        // `(-> a b c)` folds into nested flows, so three stages live across two `->`
        // nodes. A reader sees one chain; so does the panel, and so does a click.
        for source in [
            "(-> \"a\" \"b\" \"c\")",
            "(-> (-> \"a\" \"b\") \"c\")",
            "(-> \"a\" (-> \"b\" \"c\"))",
        ] {
            let mandala = Mandala::from_rebis(source).expect("the fixture parses");
            let at = |text: &str| {
                mandala
                    .nodes()
                    .iter()
                    .find(|node| node.text == text)
                    .unwrap_or_else(|| panic!("{source}: no form {text:?}"))
                    .id
            };
            let root = mandala
                .flow_chain_root(at("b"))
                .unwrap_or_else(|| panic!("{source}: the middle stage is in no chain"));
            let names = mandala
                .flow_stages(root)
                .into_iter()
                .filter_map(|id| mandala.node(id).map(|node| node.text.clone()))
                .collect::<Vec<_>>();
            assert_eq!(names, ["a", "b", "c"], "{source}: stages read wrong");
            assert_eq!(mandala.flow_stage_number(at("a")), Some((1, 3)));
            assert_eq!(mandala.flow_stage_number(at("c")), Some((3, 3)));
            // Every stage finds the same chain, from anywhere in it.
            for text in ["a", "b", "c"] {
                assert_eq!(mandala.flow_chain_root(at(text)), Some(root));
            }
        }

        // A form in no chain reports none, so the panel shows the section exactly
        // when there is something to reorder.
        let plain = Mandala::from_rebis("(\"a\" \"b\")").expect("parses");
        for node in plain.nodes() {
            assert_eq!(plain.flow_chain_root(node.id), None, "{:?}", node.form);
        }
    }

    #[test]
    fn a_chain_stage_can_be_moved_like_a_child() {
        // The stages are permuted where they SIT — the arrows keep their shape and
        // the forms exchange slots — so the order is editable as an order rather
        // than by rewriting the source.
        let mut mandala = Mandala::from_rebis("(-> \"a\" \"b\" \"c\")").expect("parses");
        let at = |mandala: &Mandala, text: &str| {
            mandala
                .nodes()
                .iter()
                .find(|node| node.text == text)
                .expect("the form")
                .id
        };
        let stage_names = |mandala: &Mandala| {
            let root = mandala.flow_chain_root(at(mandala, "a")).expect("a chain");
            mandala
                .flow_stages(root)
                .into_iter()
                .filter_map(|id| mandala.node(id).map(|node| node.text.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(stage_names(&mandala), ["a", "b", "c"]);

        // Last to first.
        let c = at(&mandala, "c");
        assert!(mandala.set_flow_stage_number(c, 1));
        assert_eq!(stage_names(&mandala), ["c", "a", "b"]);
        assert_eq!(mandala.to_rebis().unwrap(), "(-> (-> \"c\" \"a\") \"b\")");

        // Moving to where it already is changes nothing and reports so.
        assert!(!mandala.set_flow_stage_number(c, 1));
        // Out of range is refused rather than clamped.
        assert!(!mandala.set_flow_stage_number(c, 0));
        assert!(!mandala.set_flow_stage_number(c, 4));
        assert_eq!(stage_names(&mandala), ["c", "a", "b"]);

        // And back, so the operation is reversible.
        assert!(mandala.set_flow_stage_number(c, 3));
        assert_eq!(stage_names(&mandala), ["a", "b", "c"]);
        assert_eq!(mandala.to_rebis().unwrap(), "(-> (-> \"a\" \"b\") \"c\")");
    }

    #[test]
    fn a_chain_of_flows_draws_as_a_chain() {
        // `(-> (-> a b) c)` means a feeds b feeds c, so that is what it draws. The
        // outer arrow leaves the inner flow's VALUE — `b` — rather than leaving the
        // flow itself, which is not a shape and would have left the arrow starting
        // from empty canvas.
        let mandala = Mandala::from_rebis("(-> (-> \"a\" \"b\") \"c\")").unwrap();
        let named = |text: &str| {
            mandala
                .nodes()
                .iter()
                .find(|node| node.text == text)
                .unwrap_or_else(|| panic!("no form {text:?}"))
                .id
        };
        let flows = mandala
            .nodes()
            .iter()
            .filter(|node| node.form.is_flow())
            .map(|node| node.id)
            .collect::<Vec<_>>();
        assert_eq!(flows.len(), 2);
        let (outer, inner) = (flows[0], flows[1]);

        // The inner flow supplies `b` and receives at `a`.
        assert_eq!(mandala.flow_endpoint(inner, true), named("b"));
        assert_eq!(mandala.flow_endpoint(inner, false), named("a"));
        // So the outer arrow runs from `b` to `c`, and the two arrows chain.
        assert_eq!(mandala.flow_endpoint(outer, true), named("c"));
        let kids = mandala.children(outer);
        assert_eq!(mandala.flow_endpoint(kids[0], true), named("b"));
        assert_eq!(mandala.flow_endpoint(kids[1], false), named("c"));

        // Read the other way round, a backflow resolves to the mirror ends.
        let back = Mandala::from_rebis("(<- \"a\" \"b\")").unwrap();
        let flow = back
            .nodes()
            .iter()
            .find(|node| node.form.is_flow())
            .unwrap()
            .id;
        let at = |text: &str| {
            back.nodes()
                .iter()
                .find(|node| node.text == text)
                .unwrap()
                .id
        };
        assert_eq!(back.flow_endpoint(flow, true), at("a"), "(<- a b) yields a");
        assert_eq!(back.flow_endpoint(flow, false), at("b"));
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
            4,
            "the flow's two operands and both branches — the flow itself is the \
             arrow between them and claims no slot"
        );
        assert_eq!(
            m.contained_children(square.id).len(),
            4,
            "the box directly holds the mediating flow's two operands and its two \
             branches — the flow itself is the arrow between them"
        );
        assert!(interior.iter().all(|id| m.is_inlined(*id)));
        // The mediator here IS a flow, so it is not among the box's contents: it is
        // the arrow drawn between two of them. What has to stay inside the brackets
        // is the pair it routes.
        let mediator = m.mediator(square.id).expect("the square names a mediator");
        assert_eq!(
            m.node(mediator).map(|node| &node.form),
            Some(&Form::Forward)
        );
        assert!(
            m.children(mediator)
                .into_iter()
                .all(|child| m.is_inlined(child)),
            "the mediator's operands stay inside the source-written brackets"
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
            3,
            "the direct prompt and the flow's two operands — the flow itself is the \
             arrow between them and claims no slot"
        );
        assert!(interior.iter().all(|id| m.is_inlined(*id)));
        assert_eq!(
            m.contained_children(compose.id).len(),
            3,
            "the prompt plus the flow's two operands, all at one level"
        );
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
            // The circle has to contain the SHAPE, which is the centre distance plus
            // the radius of the smallest circle around it — not the corner of its
            // bounding box, which only a square actually reaches.
            let reach = (node.x - compose.x).hypot(node.y - compose.y)
                + circumscribed_reach(node.shape(), m.extent(*id));
            assert!(
                reach + MEDIATOR_PAD <= radius_x + 1e-6,
                "{:?} escaped the compose circle: reaches {reach:.1} of {radius_x:.1}",
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
                .filter(|node| !mandala.is_written_prefix(node.id))
                .all(|node| mandala.is_inlined(node.id)),
            "nothing is left loose outside a boundary"
        );
        assert_eq!(mandala.to_rebis().unwrap(), source);
    }

    #[test]
    fn structural_forms_use_their_declared_outlines() {
        assert_eq!(Form::Prompt.shape(), Shape::Hexagon);
        assert_eq!(Form::Compose.shape(), Shape::Circle);
        // The box belongs to the mediator alone: a square on the canvas can only mean
        // a mediation. The implicit top-level scope is a compose like any other.
        assert_eq!(Form::Square.shape(), Shape::Square);
        assert_eq!(Form::Program.shape(), Shape::Circle);
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
            if form.is_flow() || form == Form::Program {
                // Flow shares the plain circle with compose by design: what tells
                // them apart is the arrow drawn between the two forms inside it, and
                // a circle titled with an arrow is nothing the palette could build.
                //
                // A program shares it for a stronger reason — it IS a compose, the
                // outermost one, and the language draws no distinction between them.
                // Told apart by position: it is the boundary nothing else holds.
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
    fn a_program_is_a_compose_and_is_drawn_as_one() {
        // Nothing in the language distinguishes a program from a compose: every
        // evaluator and runtime site reads `Program(items) | Compose(items)` as one
        // case, and the only difference is that the outermost level prints without its
        // parentheses. It had a shape of its own — a triangle — which claimed a form
        // the language does not have, and cost a great deal besides: a triangle only
        // holds a round arrangement out to 0.41 of its half-width, so the drawing came
        // out roughly twenty times the area a circle needs.
        assert_eq!(Form::Program.shape(), Form::Compose.shape());
        assert_eq!(Form::Program.shape(), Shape::Circle);
        assert!(
            Form::Program.opens_indentation(),
            "and it holds what it names"
        );

        // Told apart by position and by name, not by outline: it is the boundary
        // nothing else holds, and its label appears when it is selected.
        let source = "(~ f (x) x)\n(~ g (x) x)";
        let mandala = Mandala::from_rebis(source).expect("the fixture parses");
        let program = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Program)
            .expect("two top-level forms make an implicit program");
        assert_eq!(mandala.holder(program.id), None, "nothing holds a program");
        assert_eq!(
            program.caption(),
            Form::Compose.name().replace("compose", ""),
            "a program says no more on the canvas than a bare compose does"
        );
        assert_eq!(
            program.mark(),
            "",
            "it wears no sigil, as a bare compose does not"
        );
        // And it still prints as a bare sequence rather than gaining parentheses.
        assert_eq!(mandala.to_rebis().unwrap(), source);
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
        // An indentation writes its head on its ring, leaving the interior to
        // the forms it holds.
        let f = m.add(Form::Function(vec!["x".into()]), "twice", 0.0, 0.0);
        assert_eq!(m.node(f).unwrap().caption(), "");
        assert_eq!(
            m.node(f).unwrap().mark(),
            "~ twice (x)",
            "the head, whole — but only the head, never the body it holds"
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
    fn a_program_holds_everything_it_names() {
        // The program is a boundary, not a mark floating beside the forms it names.
        // Its contents wind on the same spiral every interior uses, and since it is
        // drawn as the compose it is, the circle closes on them at their own radius —
        // where a triangle would have had to be two and a half times wider to hold the
        // same arrangement without its corners cutting through them.
        assert!(Form::Program.opens_indentation(), "a program is a boundary");

        let source = (0..7)
            .map(|index| format!("(~ f{index} (x) x)"))
            .collect::<Vec<_>>()
            .join("\n");
        let mandala = Mandala::from_rebis(&source).expect("the fixture parses");
        let program = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Program)
            .expect("an implicit program");
        let held = mandala.contained_children(program.id);
        assert_eq!(held.len(), 7, "every definition is inside it: {held:?}");

        // Every content's own outline, sampled all the way round, lies within the
        // program's circle.
        let (radius, _) = mandala.extent(program.id);
        for id in &held {
            let node = mandala.node(*id).expect("held");
            let footprint = mandala.footprint(*id);
            let reach =
                (node.x - program.x).hypot(node.y - program.y) + footprint.0.max(footprint.1);
            assert!(
                reach <= radius + 1e-6,
                "{:?} reaches {reach:.0} of the program's {radius:.0}",
                node.form
            );
        }

        // And the circle is no larger than that containment requires.
        let reach = held
            .iter()
            .filter_map(|id| {
                let node = mandala.node(*id)?;
                let footprint = mandala.footprint(*id);
                Some((node.x - program.x).hypot(node.y - program.y) + footprint.0.max(footprint.1))
            })
            .fold(0.0_f64, f64::max);
        assert!(
            (radius - reach - MEDIATOR_PAD).abs() < 1e-6,
            "the program closes at {radius:.0} where its contents reach {reach:.0}"
        );
    }

    #[test]
    fn a_file_of_definitions_draws_as_a_page_not_a_ribbon() {
        // The commonest shape of program there is: top-level definitions, all one
        // deep. The outer level used to be a circuit with nesting depth as the
        // column, so every one of them shared a column and the drawing came out a
        // ribbon — measured on the collection, eleven thousand units tall against
        // four thousand wide, readable only at six percent zoom. A spiral spends both
        // directions, so the page stays roughly square however many definitions
        // there are.
        for count in [4usize, 12, 24] {
            let source = (0..count)
                .map(|index| format!("(~ f{index} (x) x)"))
                .collect::<Vec<_>>()
                .join("\n");
            let mandala = Mandala::from_rebis(&source).expect("the fixture parses");
            let bounds = mandala.bounds().expect("the drawing has bounds");
            let (width, height) = (bounds.max_x - bounds.min_x, bounds.max_y - bounds.min_y);
            let aspect = width / height;
            assert!(
                (0.4..2.5).contains(&aspect),
                "{count} definitions drew a ribbon: {width:.0} x {height:.0} \
                 (aspect {aspect:.2})"
            );
        }

        // And the outermost forms really are on one arm rather than one column: a
        // column would put them all at the same x.
        let source = (0..8)
            .map(|index| format!("(~ g{index} (x) x)"))
            .collect::<Vec<_>>()
            .join("\n");
        let mandala = Mandala::from_rebis(&source).expect("parses");
        let outer = mandala
            .nodes()
            .iter()
            .filter(|node| matches!(node.form, Form::Function(_)))
            .collect::<Vec<_>>();
        assert_eq!(outer.len(), 8);
        let columns = outer
            .iter()
            .map(|node| (node.x / 10.0).round() as i64)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            columns.len() > 2,
            "the definitions are stacked in {} column(s), not wound on an arm",
            columns.len()
        );
    }

    #[test]
    fn formatting_keeps_hand_resized_forms_apart_and_keeps_their_sizes() {
        // Two guarantees, both older than the layout that used to provide them. The
        // top level was a circuit — depth chose the column, rows packed beneath —
        // and it reserved room for a form someone had deliberately enlarged. It is a
        // spiral now, and it has to reserve the same room: a hand-set size survives
        // formatting, and nothing formatting places overlaps anything else.
        let mut mandala = Mandala::new();
        let father = mandala.add(Form::Square, "", 0.0, 0.0);
        let upper = mandala.add(Form::Prompt, "upper", 0.0, 0.0);
        let lower = mandala.add(Form::Symbol, "lower", 0.0, 0.0);
        mandala.father_of(father, upper);
        mandala.father_of(father, lower);
        mandala.resize(father, 220.0, 90.0);
        // The children scale whole, so each one's half-extents come back in its own
        // proportions; the layout must reserve whatever that works out to.
        mandala.resize(upper, 70.0, 100.0);
        mandala.resize(lower, 80.0, 110.0);
        let sizes = [
            mandala.extent(father),
            mandala.extent(upper),
            mandala.extent(lower),
        ];

        mandala.relayout();

        // The sizes are still what they were dragged to.
        assert_eq!(
            [
                mandala.extent(father),
                mandala.extent(upper),
                mandala.extent(lower),
            ],
            sizes,
            "formatting must not undo a hand-set size on the outer level"
        );

        // And none of the three overlaps another.
        let placed = [father, upper, lower];
        for (index, left) in placed.iter().copied().enumerate() {
            for right in placed.iter().copied().skip(index + 1) {
                let (a, b) = (
                    mandala.node(left).expect("placed"),
                    mandala.node(right).expect("placed"),
                );
                let apart = (a.x - b.x).hypot(a.y - b.y);
                let needed = circumscribed_reach(a.shape(), mandala.footprint(left))
                    + circumscribed_reach(b.shape(), mandala.footprint(right));
                assert!(
                    apart >= needed - 1e-6,
                    "{:?} and {:?} overlap after formatting: {apart:.1} apart, needing \
                     {needed:.1}",
                    a.form,
                    b.form
                );
            }
        }
    }

    #[test]
    fn a_prefix_can_be_written_onto_a_form_and_taken_off_again() {
        // The palette can place `'` and `,`, but a prefix is a mark on a form,
        // not a shape you drop somewhere. Without a way to attach one, placing
        // it was a dead end: it could never become `'x`.
        // Quoted prompts, because `(x y)` would parse as a call to `x`.
        let mut m = Mandala::from_rebis("(\"a\" \"b\")").unwrap();
        let symbol = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Prompt)
            .unwrap()
            .id;
        let circle = m.holder(symbol).unwrap();
        let was = m.child_number(circle, symbol);

        let quote = m
            .wrap(symbol, Form::Quote)
            .expect("a prefix may be written");
        assert_eq!(m.written_prefix(symbol), "'");
        assert_eq!(m.to_rebis().unwrap(), "('\"a\" \"b\")");
        // The marked form keeps its place: the prefix took it over.
        assert_eq!(m.child_number(circle, quote), was);
        assert_eq!(m.prefixed_with(symbol, &Form::Quote), Some(quote));

        // And taking it off returns the drawing exactly as it was.
        let kept = m.unwrap_form(quote);
        assert_eq!(kept, vec![symbol], "the form it marked is kept");
        assert_eq!(m.written_prefix(symbol), "");
        assert_eq!(m.to_rebis().unwrap(), "(\"a\" \"b\")");
        assert_eq!(
            m.child_number(circle, symbol),
            was,
            "in its own place again"
        );
        assert_eq!(m.holder(symbol), Some(circle));
    }

    #[test]
    fn a_mark_can_be_taken_off_from_the_form_it_is_written_on() {
        // Marks come off one at a time, outermost first, and the form they were
        // written on stays exactly where it was drawn.
        let mut m = Mandala::from_rebis("(\"a\" ',\"b\")").unwrap();
        let marked = m
            .nodes()
            .iter()
            .filter(|node| node.form == Form::Prompt)
            .map(|node| node.id)
            .find(|id| !m.written_prefix(*id).is_empty())
            .expect("a form carrying marks");
        let circle = m.holder(marked).expect("the form itself holds the slot");
        assert_eq!(m.written_prefix(marked), "',");

        // The outermost mark is the top of the chain standing over the form.
        let outermost = |m: &Mandala| {
            let mut cursor = marked;
            while let Some(father) = m.father(cursor).filter(|f| m.is_written_prefix(*f)) {
                cursor = father;
            }
            cursor
        };
        assert_ne!(outermost(&m), marked, "two marks stand over it");

        m.unwrap_form(outermost(&m));
        assert_eq!(m.written_prefix(marked), ",", "the outer `'` came off");
        assert_eq!(m.holder(marked), Some(circle), "still in its boundary");

        m.unwrap_form(outermost(&m));
        assert_eq!(m.written_prefix(marked), "", "and then the inner one");
        assert_eq!(outermost(&m), marked, "nothing stands over it now");
        assert_eq!(m.holder(marked), Some(circle));
        assert_eq!(m.to_rebis().unwrap(), "(\"a\" \"b\")");
    }

    #[test]
    fn removing_a_level_keeps_what_it_held() {
        // Deleting a boundary takes its contents with it. Removing the level
        // and keeping them is a different move: the operands are spliced into
        // the hole, inheriting its place and the boundary that drew it.
        let mut m = Mandala::from_rebis("(\"a\" (\"b\" \"c\") \"d\")").unwrap();
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
        let at = m.child_number(outer, inner).unwrap();

        let kept = m.unwrap_form(inner);
        assert_eq!(kept.len(), 2, "both forms it held moved up");
        assert!(m.node(inner).is_none(), "and the level itself is gone");
        for (offset, id) in kept.iter().copied().enumerate() {
            assert_eq!(m.holder(id), Some(outer), "drawn in the boundary above");
            assert_eq!(m.child_number(outer, id), Some(at + offset), "in its place");
        }
        assert_eq!(m.to_rebis().unwrap(), "(\"a\" \"b\" \"c\" \"d\")");
    }

    #[test]
    fn a_resized_form_reserves_room_for_the_larger_mark_it_actually_draws() {
        // The canvas magnifies a sigil with the boundary it names, so the room
        // reserved has to grow with it. A flat band was right for an unresized
        // circle and short by up to six times for a resized one — which is how a
        // scaled-up `~` ended up drawn under its container's wall.
        let mut m = Mandala::new();
        let outer = m.add(Form::Compose, "", 0.0, 0.0);
        let macro_form = m.add(Form::Function(vec!["x".into()]), "pr-fetched", 0.0, 0.0);
        m.hold(outer, macro_form);

        let small = m.footprint(macro_form).0 - m.extent(macro_form).0;
        assert!(
            small >= LABEL_BAND,
            "an unresized mark reserves at least the label band, got {small}"
        );
        // A head is a line, not a character, so the room it takes follows the
        // words in it: `~ pr-fetched (x)` needs more than a bare `$` beside it.
        let bare = {
            let mut m = Mandala::new();
            let outer = m.add(Form::Compose, "", 0.0, 0.0);
            let concat = m.add(Form::Concat, "", 0.0, 0.0);
            m.hold(outer, concat);
            m.footprint(concat).0 - m.extent(concat).0
        };
        assert!(
            small > bare,
            "a named macro reserved no more room than a bare sigil: {small} vs {bare}"
        );

        // Scale the macro's circle up hard, as dragging its wall does.
        m.resize(macro_form, 900.0, 900.0);
        let band = m.footprint(macro_form).0 - m.extent(macro_form).0;
        let weight = mark_weight((
            m.extent(macro_form).0 / macro_form_base(&m, macro_form),
            m.extent(macro_form).1 / macro_form_base(&m, macro_form),
        ));
        assert!(weight > 1.0, "the mark is magnified at this size");
        assert!(
            band >= MARK_HEIGHT * weight / 2.0,
            "band {band} does not cover a mark drawn at {}",
            MARK_HEIGHT * weight
        );
        assert!(band > LABEL_BAND * 3.0, "the band did not grow: {band}");

        // And the container closes outside the mark, not through it.
        let (inner_x, inner_y) = (m.node(macro_form).unwrap().x, m.node(macro_form).unwrap().y);
        let centre = m.node(outer).unwrap();
        let reach = (inner_x - centre.x).hypot(inner_y - centre.y) + m.footprint(macro_form).0;
        assert!(
            m.extent(outer).0 >= reach,
            "the container at {} closes inside the mark's reach {reach}",
            m.extent(outer).0
        );
    }

    fn macro_form_base(m: &Mandala, id: NodeId) -> f64 {
        m.node(id).unwrap().base_extent().0
    }

    #[test]
    fn a_circle_reserves_room_for_its_ring_label_whether_or_not_it_wears_one() {
        // The mark is written centred on the outline, so half of it stands
        // outside. Room for it is reserved on every circle: a bare `( )` takes
        // the same space as a `$`, so a row of them reads as a row.
        let mut m = Mandala::new();
        let bare = m.add(Form::Compose, "", 0.0, 0.0);
        let marked = m.add(Form::Concat, "", 0.0, 0.0);
        let square = m.add(Form::Square, "", 0.0, 0.0);

        assert_eq!(
            m.footprint(bare),
            m.footprint(marked),
            "a circle with no mark must take the room one with a mark takes"
        );
        // The outline itself is untouched: what is drawn and what is clicked are
        // the same size as before.
        assert_eq!(m.extent(bare), (NODE_R, NODE_R));
        assert_eq!(m.footprint(bare).0, m.extent(bare).0 + LABEL_BAND);
        // The square writes `[ ]` with its own outline and needs no band.
        assert_eq!(m.footprint(square), m.extent(square));

        // Framing sees the label, so the mark on the outermost circle cannot
        // fall off the edge of the window.
        let bounds = m.bounds().unwrap();
        assert!(bounds.width() >= (m.extent(bare).0 + LABEL_BAND) * 2.0 - 1e-9);

        // And a boundary closes outside its contents' labels rather than through
        // them.
        let outer = m.add(Form::Compose, "", 0.0, 0.0);
        m.father_of(outer, marked);
        m.hold(outer, marked);
        assert!(
            m.extent(outer).0 >= m.footprint(marked).0,
            "the wall cut through the mark of what it holds"
        );
    }

    #[test]
    fn a_connection_attaches_to_what_is_actually_drawn() {
        // A prefix sigil is painted on the front of the form it marks, so it is
        // never a shape a line can point at. An arrow whose operand is one must
        // attach to the form underneath it — otherwise the line arrives from
        // empty canvas with a head on the end, aimed at nothing.
        let m = Mandala::from_rebis("(~ f (worker critic) '(-> ,worker ,critic))").unwrap();
        let flow = m.nodes().iter().find(|node| node.form.is_flow()).unwrap();
        let operands = m.children(flow.id);
        assert_eq!(operands.len(), 2);

        for operand in operands {
            // The operand as written is the invisible sigil...
            assert!(m.is_written_prefix(operand), "the operand is a prefix");
            // ...and the form the line must reach is the one it marks.
            let drawn = m.drawn_form(operand);
            assert_ne!(drawn, operand, "the sigil is not what is drawn");
            assert!(!m.is_written_prefix(drawn), "and what is drawn is a form");
            assert_eq!(m.node(drawn).unwrap().form, Form::Symbol);
        }

        // Resolution walks a whole chain, not one step, and a form that is
        // already drawn resolves to itself.
        let stacked = Mandala::from_rebis("(~ f (x) '(-> ',x ,x))").unwrap();
        for node in stacked.nodes() {
            let drawn = stacked.drawn_form(node.id);
            assert!(
                !stacked.is_written_prefix(drawn),
                "{:?} left a sigil",
                node.id
            );
        }
        let plain = Mandala::from_rebis("(-> \"a\" \"b\")").unwrap();
        for node in plain.nodes() {
            assert_eq!(plain.drawn_form(node.id), node.id, "nothing to resolve");
        }
    }

    #[test]
    fn a_prefix_sigil_is_written_on_its_operand_not_beside_it() {
        // `,worker` is one written thing. Drawn as a loose comma somewhere near
        // a loose diamond, the adjacency that carried the relation is gone and
        // the reader has to guess which mark belongs to which form.
        let m = Mandala::from_rebis("(,worker ,task)").unwrap();
        let circle = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose)
            .unwrap()
            .id;

        // The circle holds the two symbols. The sigils take no slot of their
        // own and are read off the front of the form they mark.
        let held = m.contained_children(circle);
        assert_eq!(held.len(), 2, "two forms inside, not four");
        for id in held {
            assert_eq!(m.node(id).unwrap().form, Form::Symbol);
            assert_eq!(m.written_prefix(id), ",");
        }
        for node in m.nodes() {
            if node.form.is_prefix_sigil() {
                assert!(m.is_written_prefix(node.id));
                assert_eq!(m.holder(node.id), None, "a prefix claims no slot");
            }
        }

        // Stacked prefixes read outermost first, exactly as written.
        let m = Mandala::from_rebis("(',x)").unwrap();
        let symbol = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Symbol)
            .unwrap()
            .id;
        assert_eq!(m.written_prefix(symbol), "',");
        assert_eq!(m.to_rebis().unwrap(), "(',x)");

        // A quoted boundary wears its prefix too, and keeps its own contents.
        let m = Mandala::from_rebis("('([,x] ,x))").unwrap();
        let square = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Square)
            .unwrap()
            .id;
        assert_eq!(m.written_prefix(square), "'");
        assert_eq!(m.contained_children(square).len(), 2);
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

    /// Where a mark is written, how big it is, and what it says.
    struct WrittenMark {
        centre: (f64, f64),
        half: (f64, f64),
        text: String,
    }

    /// The box a mark actually occupies on the canvas: centred on the boundary's
    /// topmost point, as wide as its text at the size it is drawn.
    fn written_mark_box(mandala: &Mandala, id: NodeId) -> Option<WrittenMark> {
        let node = mandala.node(id)?;
        let mark = format!("{}{}", mandala.written_prefix(id), node.mark());
        if mark.is_empty() || mandala.is_written_prefix(id) {
            return None;
        }
        let extent = mandala.extent(id);
        let base = node.base_extent();
        let height = MARK_HEIGHT
            * mark_weight((
                extent.0 / base.0.max(f64::EPSILON),
                extent.1 / base.1.max(f64::EPSILON),
            ));
        #[allow(clippy::cast_precision_loss)]
        let half_w = mark.chars().count() as f64 * height * MARK_ADVANCE / 2.0;
        Some(WrittenMark {
            centre: (node.x, node.y - extent.1),
            half: (half_w, height / 2.0),
            text: mark,
        })
    }

    #[test]
    fn shrinking_a_form_leaves_the_walls_around_it_where_they_are() {
        // A boundary's size is derived from what it holds, so making one form smaller
        // pulled its container in after it, and that container's container, out to
        // the page — one circle made smaller rearranged the whole drawing, and only
        // ever the form that happened to sit farthest out, which reads as a glitch
        // rather than as a rule.
        //
        // Growing is different and stays: a wall has to keep containing what it
        // holds. So a hand gesture may push a wall out and never pull it in, and
        // `format mandala` is what hands a boundary back to its contents.
        let mut mandala = Mandala::from_rebis("(((\"a\")) ((\"b\")) ((\"c\")))").unwrap();
        mandala.relayout();
        let root = mandala
            .nodes()
            .iter()
            .find(|node| mandala.holder(node.id).is_none())
            .expect("a root")
            .id;
        let held = mandala.contained_children(root);
        assert_eq!(held.len(), 3);

        // Whichever content sits farthest out is the one that used to drag the wall.
        let centre = mandala.node(root).map(|node| (node.x, node.y)).unwrap();
        let farthest = held
            .iter()
            .copied()
            .max_by(|left, right| {
                let reach = |id: NodeId| {
                    let node = mandala.node(id).unwrap();
                    (node.x - centre.0).hypot(node.y - centre.1)
                };
                reach(*left).total_cmp(&reach(*right))
            })
            .expect("a farthest content");

        // Growing carries the wall out, or a form would escape the boundary that
        // owns it. A form can never be drawn below its own base size, so this is
        // also the only way to have something to shrink.
        let wall = mandala.extent(root).0;
        let natural = mandala.extent(farthest).0;
        mandala.resize_group(farthest, natural * 4.0, natural * 4.0);
        let opened = mandala.extent(root).0;
        assert!(
            opened > wall,
            "a growing content must push its wall out: {wall:.1} -> {opened:.1}"
        );

        // Shrinking it back does NOT bring the wall in with it.
        let grown = mandala.extent(farthest).0;
        mandala.resize_group(farthest, natural, natural);
        assert!(
            mandala.extent(farthest).0 < grown,
            "the form did not actually get smaller"
        );
        assert!(
            (mandala.extent(root).0 - opened).abs() < 1e-6,
            "the wall came in from {opened:.1} to {:.1} because a content shrank",
            mandala.extent(root).0
        );

        // And formatting hands every wall back to its contents, so the hold is not
        // permanent.
        let held_wall = mandala.extent(root).0;
        assert!(held_wall > wall, "the wall is still being held open");
        mandala.relayout();
        assert!(
            mandala.extent(root).0 < held_wall,
            "formatting must tighten a wall that was held open: still {:.1}",
            mandala.extent(root).0
        );
        assert!(
            mandala.nodes().iter().all(|node| node.floor.is_none()),
            "formatting must drop every floor"
        );
    }

    #[test]
    fn a_boundary_never_writes_its_mark_over_something_it_holds() {
        // A mark is centred on the outline, so half of it hangs INSIDE the ring. With
        // one content — which sits at the middle — a boundary's radius exceeded its
        // content's by only the content's band plus a pad, so the two marks sat a few
        // units apart and ran through each other. The reserve has to cover the
        // container's own mark, and count a bare prefix as one: a quoted `( )` wears
        // `'` and nothing else.
        for source in [
            // A macro whose body is a quoted conditional: `~` outside, `'%` within.
            "(~ f (a b) '(% a b a))",
            // Two levels of prefix on nested boundaries.
            "(~ g (x) '((\"one\") ,x))",
            // A deep chain, every level marked.
            "(~ h (x) '(% ($ \"a\" \"b\") (^ (\"c\")) x))",
        ] {
            let mut mandala = Mandala::from_rebis(source).expect("the fixture parses");
            mandala.relayout();
            let boxes: Vec<_> = mandala
                .nodes()
                .iter()
                .filter_map(|node| written_mark_box(&mandala, node.id))
                .collect();
            assert!(boxes.len() >= 2, "{source}: needs marks to compare");
            for (index, left) in boxes.iter().enumerate() {
                for right in boxes.iter().skip(index + 1) {
                    let (dx, dy) = (
                        (left.centre.0 - right.centre.0).abs(),
                        (left.centre.1 - right.centre.1).abs(),
                    );
                    assert!(
                        dx >= left.half.0 + right.half.0 || dy >= left.half.1 + right.half.1,
                        "{source}: {:?} and {:?} overlap — {dx:.0} apart across \
                         (needs {:.0}) and {dy:.0} up (needs {:.0})",
                        left.text,
                        right.text,
                        left.half.0 + right.half.0,
                        left.half.1 + right.half.1
                    );
                }
            }
        }
    }

    #[test]
    fn a_circles_written_name_clears_its_neighbours_names() {
        // A mark is not always one sigil: a call wears its callee's NAME, written
        // across the top of its circle, reaching much further sideways than up. Room
        // reserved from the glyph's height alone gave a seventeen-character name the
        // same sixteen units a `$` gets, and neighbouring names ran through one
        // another.
        let source = "((std-with-evidence \"a\") (std-final-only \"b\") \
                      (pr-fetched \"c\") (std-reconciled \"d\"))";
        let mut mandala = Mandala::from_rebis(source).expect("the fixture parses");
        mandala.relayout();
        let root = mandala
            .nodes()
            .iter()
            .find(|node| mandala.holder(node.id).is_none())
            .expect("a root")
            .id;
        let held = mandala.contained_children(root);
        assert!(held.len() >= 4, "four named circles: {held:?}");

        // Where each name is actually written: centred on its circle's topmost
        // point, as wide as its text.
        let written = |id: NodeId| {
            let node = mandala.node(id).expect("the form");
            let mark = format!("{}{}", mandala.written_prefix(id), node.mark());
            let extent = mandala.extent(id);
            let height = MARK_HEIGHT;
            #[allow(clippy::cast_precision_loss)]
            let half_w = mark.chars().count() as f64 * height * MARK_ADVANCE / 2.0;
            ((node.x, node.y - extent.1), (half_w, height / 2.0), mark)
        };
        for (index, left) in held.iter().copied().enumerate() {
            let (a, ha, name_a) = written(left);
            if name_a.is_empty() {
                continue;
            }
            for right in held.iter().copied().skip(index + 1) {
                let (b, hb, name_b) = written(right);
                if name_b.is_empty() {
                    continue;
                }
                assert!(
                    (a.0 - b.0).abs() >= ha.0 + hb.0 || (a.1 - b.1).abs() >= ha.1 + hb.1,
                    "the names {name_a:?} and {name_b:?} overlap"
                );
            }
        }

        // And a longer name really does claim more room than a shorter one — the
        // whole point, and what a height-only band could not express.
        let band = |id: NodeId| mandala.footprint(id).0 - mandala.extent(id).0;
        let mut named: Vec<(usize, f64)> = held
            .iter()
            .filter_map(|id| {
                let node = mandala.node(*id)?;
                let length = node.mark().chars().count();
                (length > 0).then(|| (length, band(*id)))
            })
            .collect();
        named.sort_by_key(|(length, _)| *length);
        assert!(named.len() >= 2, "at least two named circles to compare");
        assert!(
            named[0].1 < named[named.len() - 1].1,
            "a {}-character name reserved {:.1} and a {}-character one {:.1}",
            named[0].0,
            named[0].1,
            named[named.len() - 1].0,
            named[named.len() - 1].1
        );
    }

    #[test]
    fn a_boundary_closes_one_pad_outside_what_it_holds() {
        // "Least possible space" ends at the wall: a circle stands exactly
        // MEDIATOR_PAD outside the farthest thing it holds, and not a unit more.
        // What used to be more was the corner of a round form's bounding box —
        // reached for as though every shape were a box, which cost every nesting
        // level a further √2.
        for source in [
            "((# a) (# b) (# c) (# d) (# e) (# f))",
            "((\"x\") (\"y\") (\"z\"))",
            "((\"one\" \"two\") (\"three\" \"four\") x)",
        ] {
            let mut mandala = Mandala::from_rebis(source).expect("the fixture parses");
            mandala.relayout();
            let root = mandala
                .nodes()
                .iter()
                .find(|node| mandala.holder(node.id).is_none())
                .expect("a root")
                .id;
            let centre = mandala.node(root).map(|node| (node.x, node.y)).unwrap();
            let reach = mandala
                .contained_children(root)
                .into_iter()
                .filter_map(|id| {
                    let node = mandala.node(id)?;
                    Some(
                        (node.x - centre.0).hypot(node.y - centre.1)
                            + circumscribed_reach(node.shape(), mandala.footprint(id)),
                    )
                })
                .fold(0.0_f64, f64::max);
            let wall = mandala.extent(root).0;
            assert!(
                (wall - reach - MEDIATOR_PAD).abs() < 1e-6,
                "{source}: wall at {wall:.1}, contents reach {reach:.1} — slack is \
                 {:.1}, should be exactly {MEDIATOR_PAD}",
                wall - reach
            );
        }
    }

    #[test]
    fn a_box_lays_its_contents_on_the_golden_section() {
        // The figure follows the boundary: rows and columns inside something with
        // sides, a curve inside something without. The column count is chosen so the
        // whole comes out as near a golden rectangle as integer rows and a fixed cell
        // allow — exactly φ is not reachable, since the aspect can only be the cell's
        // own times a ratio of whole numbers, and padding it out to φ would buy the
        // proportion with empty space.
        for count in [2usize, 3, 4, 6, 9, 12] {
            let branches = (0..count)
                .map(|index| format!("\"b{index}\""))
                .collect::<Vec<_>>()
                .join(" ");
            let mut mandala =
                Mandala::from_rebis(&format!("([\"m\"] {branches})")).expect("the fixture parses");
            mandala.relayout();
            let square = mandala
                .nodes()
                .iter()
                .find(|node| node.form == Form::Square)
                .expect("the box")
                .id;
            let held = mandala.contained_children(square);
            assert_eq!(held.len(), count + 1, "the mediator is held too");

            // Contents sit on a grid: every form shares a row or a column with
            // another, and no two overlap.
            let at = |id: NodeId| {
                let node = mandala.node(id).unwrap();
                (node.x, node.y)
            };
            let rows: std::collections::BTreeSet<i64> = held
                .iter()
                .map(|id| (at(*id).1 * 1000.0).round() as i64)
                .collect();
            let columns: std::collections::BTreeSet<i64> = held
                .iter()
                .map(|id| (at(*id).0 * 1000.0).round() as i64)
                .collect();
            assert!(
                rows.len() * columns.len() >= held.len(),
                "{count}: contents are not on a grid — {} rows × {} columns for {} \
                 forms",
                rows.len(),
                columns.len(),
                held.len()
            );
            for (index, left) in held.iter().copied().enumerate() {
                for right in held.iter().copied().skip(index + 1) {
                    let (a, b) = (at(left), at(right));
                    let (ea, eb) = (mandala.footprint(left), mandala.footprint(right));
                    assert!(
                        (a.0 - b.0).abs() >= ea.0 + eb.0 + CONTENT_GAP - 1e-6
                            || (a.1 - b.1).abs() >= ea.1 + eb.1 + CONTENT_GAP - 1e-6,
                        "{count}: grid forms {left:?} and {right:?} overlap"
                    );
                }
            }

            // And the box is landscape, near φ — never a tall stack, because a
            // mediator and its branches read across.
            let extent = mandala.extent(square);
            let aspect = extent.0 / extent.1;
            assert!(
                aspect > 1.0,
                "{count}: the box came out taller than wide ({aspect:.2})"
            );
            assert!(
                (aspect.ln() - PHI.ln()).abs() < 0.5,
                "{count}: aspect {aspect:.2} is nowhere near φ"
            );
        }
    }

    #[test]
    fn many_equal_forms_pack_tightly_instead_of_spreading() {
        // The failure this guards: at a constant angular step, two neighbours at
        // radius `R` sit `R·step` apart, so the innermost pair sets the scale and
        // every form outside it is handed far more room than it needs. Twenty-four
        // identical imports needed a circle SIXTY-TWO times an import's own radius,
        // and the ratio grew with the count rather than with its square root — so
        // the more a boundary held, the emptier it looked.
        let ratio = |count: usize| {
            let source = format!(
                "({})",
                (0..count)
                    .map(|index| format!("(# m{index})"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let mut mandala = Mandala::from_rebis(&source).expect("the fixture parses");
            mandala.relayout();
            let root = mandala
                .nodes()
                .iter()
                .find(|node| mandala.holder(node.id).is_none())
                .expect("the group")
                .id;
            let held = mandala.contained_children(root);
            assert_eq!(held.len(), count);
            let leaf = mandala.extent(held[0]).0;
            mandala.extent(root).0 / leaf
        };

        // Area grows with the count, so the RADIUS may only grow with its square
        // root. A hexagonal packing of `n` equal discs needs about 1.1·√n of their
        // radius; a spiral cannot reach that, and pays the content gap besides, so
        // the bound here is 1.7·√n. Linear growth blows it at once.
        #[allow(clippy::cast_precision_loss)]
        for count in [4usize, 8, 16, 24, 40] {
            let seen = ratio(count);
            let bound = 1.7 * (count as f64).sqrt();
            assert!(
                seen < bound,
                "{count} equal forms needed {seen:.1}× their own radius, over the \
                 √n bound of {bound:.1}× — the interior is spreading, not packing"
            );
        }
        // And the growth really is sub-linear: six times the forms must not cost
        // anything like six times the radius.
        let (few, many) = (ratio(4), ratio(24));
        assert!(
            many / few < 2.6,
            "6× the forms cost {:.1}× the radius; √6 is 2.4×",
            many / few
        );
    }

    #[test]
    fn brothers_are_drawn_at_one_size_and_the_figure_settles() {
        // Peers inside one boundary come out equal, so size stops carrying
        // accidental information at a level. Equal within a KIND: a boundary
        // matches the boundaries beside it and a bare form matches the bare forms,
        // because a boundary's size is derived from what it holds and dragging a
        // symbol up to a loaded circle's width compounds outward through every
        // level above it.
        let source = "((\"a\" \"b\" (\"deep\" \"deeper\" \"deepest\")) \"c\" (\"x\"))";
        let mut m = Mandala::from_rebis(source).expect("the fixture parses");
        // The per-level reading, which is what this rule belongs to: under
        // `Sizing::Even` there are no levels to equalise, by design.
        m.set_sizing(Sizing::ByLevel);
        m.relayout();

        let root = m
            .nodes()
            .iter()
            .find(|node| m.holder(node.id).is_none() && node.form == Form::Compose)
            .expect("the outermost group")
            .id;
        let brothers = m.contained_children(root);
        assert!(brothers.len() >= 3, "the fixture has several brothers");

        let mut kinds: std::collections::BTreeMap<bool, Vec<(f64, f64)>> =
            std::collections::BTreeMap::new();
        for id in &brothers {
            kinds
                .entry(!m.contained_children(*id).is_empty())
                .or_default()
                .push(m.extent(*id));
        }
        assert_eq!(
            kinds.len(),
            2,
            "the fixture mixes boundaries and bare forms"
        );
        for (holds, sizes) in &kinds {
            let first = sizes[0];
            assert!(
                sizes
                    .iter()
                    .all(|size| (size.0 - first.0).abs() < 1e-6 && (size.1 - first.1).abs() < 1e-6),
                "brothers that {} are drawn at mixed sizes: {sizes:?}",
                if *holds {
                    "hold something"
                } else {
                    "hold nothing"
                }
            );
        }

        // Equalising reads a fit that it also feeds — a raised sibling raises its
        // parent's fit — so the rule has to settle, or every press of
        // `format mandala` would inflate the page a little more.
        let area = |m: &Mandala| {
            let bounds = m.bounds().expect("the drawing has bounds");
            (bounds.max_x - bounds.min_x) * (bounds.max_y - bounds.min_y)
        };
        let settled = area(&m);
        for pass in 1..4 {
            m.relayout();
            let now = area(&m);
            assert!(
                (now - settled).abs() / settled < 1e-9,
                "format {pass} grew the drawing: {settled} -> {now}"
            );
        }
    }

    #[test]
    fn a_boundary_given_more_wall_than_it_needs_hands_it_to_its_contents() {
        // Brothers are drawn at one size, so the circle holding a single form
        // gets the same wall as the crowded one beside it. What it holds has to
        // grow into that wall — a speck alone in a large circle says only that
        // some other circle was large.
        let source = "((\"a\" \"b\" \"c\" \"d\" \"e\") (\"x\"))";
        let mut m = Mandala::from_rebis(source).expect("the fixture parses");
        // Filling a wall is a per-level move: it exists because that reading
        // hands a boundary more wall than it needs. `Sizing::Even` gives it
        // exactly the wall it needs, so there is nothing to fill.
        m.set_sizing(Sizing::ByLevel);
        let alone = |m: &Mandala| {
            m.nodes()
                .iter()
                .find(|node| node.form == Form::Compose && m.contained_children(node.id).len() == 1)
                .expect("one circle holds a single form")
                .id
        };
        let lonely = alone(&m);
        let inner = m.contained_children(lonely)[0];
        let base = m.node(inner).expect("the held form").base_extent();

        m.relayout();

        let grown = m.extent(inner);
        assert!(
            grown.0 > base.0 * 1.5,
            "the lone form stayed a speck: {base:?} -> {grown:?}"
        );
        // Filled, not merely enlarged: the wall now rests on what it holds, so
        // there is no slack left between them.
        let (fit, wall) = (m.fit_extent(lonely), m.extent(lonely));
        assert!(
            (wall.0 - fit.0) / wall.0 < 0.01 && (wall.1 - fit.1) / wall.1 < 0.01,
            "the wall stands off its contents: fit {fit:?}, wall {wall:?}"
        );
        // And the contents are scaled, never stretched.
        assert!(
            ((grown.0 / base.0) - (grown.1 / base.1)).abs() < 1e-6,
            "the held form was distorted to fit: {base:?} -> {grown:?}"
        );
    }

    #[test]
    fn a_dream_draws_as_a_circle_and_returns_to_its_own_source() {
        let source = "(-> (! (research \"retry queues\")) \"now design from that\")";
        let m = Mandala::from_rebis(source).expect("the fixture parses");
        let dream = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Dream)
            .expect("the dream is drawn");
        // An indentation like every other parenthesised form, wearing the sigil
        // it was written with.
        assert!(dream.form.opens_indentation());
        assert_eq!(dream.shape(), Shape::Circle);
        assert_eq!(dream.mark(), "!");
        assert_eq!(dream.caption(), "", "its interior belongs to what it holds");
        // Exactly one operand, because a dream keeps one answer.
        assert_eq!(dream.form.arity(), Arity::Exactly(1));
        assert_eq!(m.children(dream.id).len(), 1);
        assert_eq!(m.to_rebis().unwrap(), source);
    }

    #[test]
    fn a_page_of_several_forms_writes_itself_out_while_it_is_still_several() {
        // Every drawing is several roots on its way to being one: a form is a
        // root from the moment it is placed until it is linked. The panel has
        // to keep writing the program out through that, or it goes quiet for
        // the whole of the gesture that is creating it.
        let mut m = Mandala::new();
        let first = m.add(Form::Prompt, "draft the plan", 0.0, 0.0);
        assert_eq!(m.to_rebis_page().unwrap(), "\"draft the plan\"");

        let second = m.add(Form::Symbol, "review", 200.0, 0.0);
        assert!(
            m.to_rebis().is_err(),
            "two roots are still not one expression"
        );
        assert_eq!(
            m.to_rebis_page().unwrap(),
            "\"draft the plan\"\nreview",
            "the page stopped writing itself out when it grew a second root"
        );

        // Linked, the page and the expression agree again.
        let group = m.add(Form::Compose, "", 100.0, 0.0);
        m.father_of(group, first);
        m.father_of(group, second);
        assert_eq!(m.to_rebis_page().unwrap(), m.to_rebis().unwrap());

        // And what is genuinely wrong stays wrong.
        let mut cyclic = Mandala::new();
        let a = cyclic.add(Form::Compose, "", 0.0, 0.0);
        let b = cyclic.add(Form::Compose, "", 0.0, 0.0);
        cyclic.father_of(a, b);
        cyclic.father_of(b, a);
        assert!(cyclic.to_rebis_page().is_err(), "a loop is still a loop");
    }

    #[test]
    fn whole_text_grows_a_form_until_its_own_words_fit_inside_it() {
        let sentence = "a prompt long enough that it needs several lines of its own";
        let mut m = Mandala::from_rebis(&format!("({sentence:?} x)")).expect("the fixture parses");
        let prompt = m
            .nodes()
            .iter()
            .find(|node| node.form == Form::Prompt)
            .expect("the prompt is drawn")
            .id;
        let token = m.extent(prompt);

        assert!(m.set_legend(Legend::Whole), "the legend did not change");
        m.relayout();
        let whole = m.extent(prompt);
        assert!(
            whole.0 > token.0 * 2.0,
            "the form did not grow for its text: {token:?} -> {whole:?}"
        );

        // Grown by enough, and demonstrably: the room the renderer will set type
        // in has to cover the block that type makes.
        let lines = wrap_caption(sentence, LABEL_WRAP);
        let columns = lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0) as f64;
        let area = caption_area(m.node(prompt).unwrap().shape());
        assert!(lines.len() > 1, "the fixture does not wrap: {lines:?}");
        assert!(
            whole.0 * area.width >= columns * LABEL_HEIGHT * LABEL_ADVANCE - 1e-6,
            "the text is wider than the room it was given"
        );
        assert!(
            whole.1 * area.height >= lines.len() as f64 * LABEL_HEIGHT * LABEL_LEADING - 1e-6,
            "the text is taller than the room it was given"
        );

        // And it is a setting, not a one-way door.
        assert!(m.set_legend(Legend::Token));
        m.relayout();
        assert_eq!(m.extent(prompt), token, "going back did not go back");
    }

    #[test]
    fn evening_the_sizes_draws_a_deep_form_like_a_shallow_one() {
        // A program with one heavy branch and several light ones, nested three
        // deep: the shape that makes the per-level reading expensive.
        let source = "(~ objective () \"Research and implement a falsifiable foundation for a new \
                      architecture\")\n(~ tiny () x)\n(~ deep () (((a b) (c d)) ((\"inner note\") e)))";
        let measure = |sizing| {
            let mut m = Mandala::from_rebis(source).expect("the fixture parses");
            m.set_sizing(sizing);
            m.relayout();
            let leaves: Vec<(f64, f64)> = m
                .nodes()
                .iter()
                .map(|node| node.id)
                .filter(|id| m.contained_children(*id).is_empty())
                .map(|id| m.extent(id))
                .collect();
            let smallest = leaves.iter().fold(f64::MAX, |least, e| least.min(e.0));
            let largest = leaves.iter().fold(0.0_f64, |most, e| most.max(e.0));
            let bounds = m.bounds().expect("the drawing has bounds");
            let page = (bounds.max_x - bounds.min_x).max(bounds.max_y - bounds.min_y);
            (smallest, largest, page)
        };
        let (even_small, even_large, even_page) = measure(Sizing::Even);
        let (level_small, level_large, level_page) = measure(Sizing::ByLevel);

        // Evened: what is left of the spread is the shapes' own proportions — a
        // hexagon is wider than a diamond at the same size — never depth.
        assert!(
            even_large / even_small < 1.5,
            "evened sizes still vary by {:.1}×: {even_small}..{even_large}",
            even_large / even_small
        );
        assert!(
            level_large / level_small > 3.0,
            "the fixture does not exercise the per-level spread: {level_small}..{level_large}"
        );

        // And the page pays for it, which is the point: one magnification reads
        // the whole drawing instead of one level of it.
        assert!(
            even_page < level_page * 0.75,
            "evening did not compact the page: {even_page:.0} vs {level_page:.0}"
        );
        assert!(
            even_page / even_small < level_page / level_small * 0.6,
            "the smallest form is no larger a share of the page than before"
        );
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

        // Eight brothers come out at ONE size, and — since none of them holds
        // anything — that size is the plain size a form is drawn at. Nothing gains
        // slack for sitting further along the arm.
        let drawn = sizes(&m);
        assert_eq!(drawn.len(), 8);
        assert!(
            drawn
                .iter()
                .all(|extent| (extent.0 - NODE_R).abs() < 1e-6
                    && (extent.1 - NODE_RY).abs() < 1e-6),
            "brothers holding nothing are drawn at their own size: {drawn:?}"
        );

        // A form enlarged by hand is brought back to what it needs, and the
        // boundary closes exactly on its contents afterwards.
        //
        // Not "smaller than before": with the arm stepping a quarter turn per
        // form, the span of the spiral dominates a boundary's size, so one form's
        // width no longer decides it. What formatting still guarantees is that the
        // boundary is no larger than what it holds requires.
        let held = m.contained_children(circle)[3];
        m.resize(held, NODE_R * 6.0, NODE_RY * 6.0);
        m.relayout();
        assert_eq!(sizes(&m), drawn, "formatting must undo a hand-set size");
        let (fit, drawn_extent) = (m.fit_extent(circle), m.extent(circle));
        assert!(
            (drawn_extent.0 - fit.0).abs() < 1e-6,
            "the boundary must close on its contents: drawn {drawn_extent:?}, fit {fit:?}"
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

        // No two forms overlap, and the measure is the CIRCUMSCRIBED radius: every
        // shape here is inscribed in its own bounds, so a circle of the wider half
        // contains it. Clearing them as boxes instead would demand a diagonal
        // neighbour stand 1.4× further off than it has to, which is most of what
        // made a boundary bigger than its contents.
        let reach = |id: NodeId| {
            let node = mandala.node(id).unwrap();
            circumscribed_reach(node.shape(), mandala.footprint(id))
        };
        let placed = |id: NodeId| {
            let node = mandala.node(id).unwrap();
            (node.x, node.y)
        };
        for (index, left) in items.iter().copied().enumerate() {
            for right in items.iter().copied().skip(index + 1) {
                let (a, b) = (placed(left), placed(right));
                let apart = (a.0 - b.0).hypot(a.1 - b.1);
                assert!(
                    apart >= reach(left) + reach(right) + CONTENT_GAP - 1e-3,
                    "spiral forms {left:?} and {right:?} overlap: {apart:.1} apart, \
                     needing {:.1}",
                    reach(left) + reach(right) + CONTENT_GAP
                );
            }
        }

        // And no form stands FURTHER off than it has to: each one sits at exactly
        // the clearance its own size and its neighbour's demand, to the last unit.
        // That is the whole of "least possible space" — the arm advances by what
        // each pair needs, so a small form takes a small step and no form is handed
        // room another form's size. An arm opened by one global scale until the
        // worst pair fit is what this replaced, and it cost a boundary holding two
        // dozen small forms twelve times an item's radius where a tight packing
        // needs eight.
        for pair in items.windows(2) {
            let (a, b) = (placed(pair[0]), placed(pair[1]));
            let apart = (a.0 - b.0).hypot(a.1 - b.1);
            let needed = reach(pair[0]) + reach(pair[1]) + CONTENT_GAP;
            assert!(
                apart <= needed * 1.001,
                "consecutive forms sit {apart:.1} apart, needing only {needed:.1}"
            );
        }

        // The arm winds outward: one spiral, not a ring or a scatter. Measured from
        // the FIRST form, which is where the curve starts — the boundary's own
        // centre is not the spiral's, because the arrangement is re-centred on its
        // own bounds so it sits squarely inside the circle drawn around it.
        let origin = placed(items[0]);
        let radius = |id: NodeId| {
            let at = placed(id);
            (at.0 - origin.0).hypot(at.1 - origin.1)
        };
        for pair in items.windows(2) {
            assert!(
                radius(pair[1]) > radius(pair[0]) - 1e-6,
                "the arm turned back inward at {:?}",
                pair[1]
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
        // `'` is written on the front of what it quotes, not held beside it.
        let quote = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Quote)
            .unwrap()
            .id;
        assert!(mandala.is_written_prefix(quote));
        assert_eq!(mandala.holder(quote), None, "a prefix claims no slot");
        let quoted = mandala.children(quote)[0];
        assert_eq!(
            mandala.written_prefix(quoted),
            "'",
            "and is read on the form it marks"
        );
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
    fn hit_finds_a_circle_only_inside_its_disc() {
        // The outline is the hit region, so a corner of the bounding box misses. That
        // is what makes a circle's whole circumference draggable for a resize without
        // the empty corners swallowing clicks meant for what is behind them.
        let mut m = Mandala::new();
        let a = m.add(Form::Compose, "", 100.0, 100.0);
        assert_eq!(m.hit(100.0, 100.0), Some(a));
        assert_eq!(m.hit(100.0 + NODE_R * 0.7, 100.0 + NODE_R * 0.7), Some(a));
        // The corner of the box the circle sits in is outside the circle.
        assert_eq!(m.hit(100.0 + NODE_R * 0.95, 100.0 + NODE_R * 0.95), None);
        assert_eq!(m.hit(100.0, 100.0 + NODE_R + 1.0), None);
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
    fn nested_container_surfaces_are_painted_outer_before_inner() {
        // Insert the inner circle first on purpose. Containment, not creation
        // order, decides the z-order of surfaces.
        let mut mandala = Mandala::new();
        let circle = mandala.add(Form::Compose, "", 0.0, 0.0);
        let brace = mandala.add(Form::Imaginary, "", 0.0, 0.0);
        let root = mandala.add(Form::Program, "", 0.0, 0.0);
        assert!(mandala.hold(root, brace));
        assert!(mandala.hold(brace, circle));

        let order = mandala.paint_order(true);
        let outer = order
            .iter()
            .position(|id| *id == brace)
            .expect("outer brace is painted");
        let inner = order
            .iter()
            .position(|id| *id == circle)
            .expect("inner circle is painted");
        assert!(outer < inner, "outer surface must precede inner: {order:?}");
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
    /// Every recent operator survives being drawn and read back.
    ///
    /// The mandala is one-to-one with the language, so a form that draws but
    /// does not re-read is one the canvas can lose — and the failure is
    /// silent: a program opened on the canvas and saved comes back changed.
    ///
    /// Written for the numeric plane and immediately found four other forms
    /// with the same gap. `=` had NO equivalence arm at all, so a binding has
    /// never round-tripped; `<>`, `><`, `*` and `@` were each missing too. The
    /// list below is deliberately every one of them.
    #[test]
    fn every_operator_round_trips_through_the_canvas() {
        for source in [
            "|($ 2 3)|",
            "(= n |($ 2 3)| |($ n 1)|)",
            "|([sum] (/ 9 38) (^ 7))|",
            r#"($ "read " <>)"#,
            r#"(>< "write a program")"#,
            r#"(* "topic" "correct it")"#,
            r#"(@ "rule" (-> "a" "b"))"#,
            r#"{"speculate" "conclude"}"#,
            r#"(+ "framing" "work")"#,
            // Syntax as a value: a held program, and the empty list. The
            // canvas has to draw what a program can now hold, or a run that
            // works is one nobody can see.
            r#"(= p '("hi") (p))"#,
            "'()",
            r#"($ "e=" '())"#,
        ] {
            let mandala = Mandala::from_rebis(source).expect("draw");
            let back = match mandala.to_rebis_page() {
                Ok(back) => back,
                Err(error) => panic!("{source} did not read back: {error:?}"),
            };
            assert_eq!(
                back.replace(char::is_whitespace, ""),
                source.replace(char::is_whitespace, ""),
                "the canvas changed the program:\n  in  {source}\n  out {back}"
            );
        }
    }
}
