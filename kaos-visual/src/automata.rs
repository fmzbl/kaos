//! A run, rendered as the cellular automaton it generates.
//!
//! This is not a graph of the program. A graph would show you the shape you
//! already wrote. This takes two things a run actually produces — the
//! **geometry** of the Rebis expression that executed, and the **bytes** of the
//! prompts and answers that crossed it — and uses them to build and then run an
//! automaton.
//!
//! # The lattice comes from the program
//!
//! Which cells are neighbours is decided by what the code is, not by a layout
//! algorithm:
//!
//!   · a square's branches ring their mediator, and — matching the language's own
//!     semantics — branch cells are NOT adjacent to each other, because square
//!     branches cannot observe one another;
//!   · an arrow chain is a directed line, each cell feeding the next;
//!   · a group's children sit side by side and are mutually non-adjacent,
//!     because a group is sequential composition, not flow;
//!   · nesting depth groups the cells, so each abstraction level of the code is
//!     one contiguous arc of the figure.
//!
//! # The rule comes from the model
//!
//! Cell states are seeded from prompt bytes and driven toward answer bytes, and
//! the 512-entry transition table IS the answer's bytes, stretched to the full
//! range. Its entropy sets the mixing rate. So the automaton is not decoration
//! applied to a run — the run *computes* it, and the same program on a different
//! model produces a visibly different figure because the table came out of that
//! model's tokens.
//!
//! That is the claim worth making here: the figure is a fingerprint of one
//! model's output laid over the shape of the program that elicited it. A terse,
//! low-entropy answer settles into smooth bands; a sprawling one turns to static.
//!
//! # The figure is a spacetime, not a diagram
//!
//! [`Automaton::compose`] holds the composition and documents it in full. In
//! short: **angle is space** (one sector per cell, grouped by depth), **radius is
//! time** (one ring per generation, oldest at the centre, newest at the rim), and
//! **colour is the byte** that cell held in that generation. The disc therefore
//! grows outward as the run computes, and every pixel in it is a number the run
//! produced.
//!
//! # What is honest about it
//!
//! Nothing here is a measurement of quality. A beautiful automaton is not a
//! good reading — the entropy of an answer says how varied its bytes were, not
//! whether it was right. This view is for watching a run's *character*, and for
//! seeing at a glance where a gate refused and killed a region. It is not
//! evidence, and the pane says so on screen.

use std::collections::BTreeMap;

use rebis_lang::Expr;

/// How a cell came to exist — which part of the program it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    /// A quoted prompt: the only form that fires a model.
    Prompt,
    /// A mediator at the centre of a square's ring.
    Mediator,
    /// A branch of a square. Isolated from its siblings by construction.
    Branch,
    /// A stage of an arrow chain.
    Stage,
    /// A lazy gate's condition. Kills its region when it answers 0.
    Gate,
    /// Structure: a group, a macro body, a symbol.
    Structure,
}

/// Which byte stream is being written into the binary halo around the
/// spacetime.  Keeping this semantic label in the composition means the pane
/// can give prompt bits and response bits different visual roles without
/// changing the automaton's geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryStream {
    Prompt,
    Response,
}

impl Site {
    /// Weight in the neighbourhood sum. A prompt is the loud thing in a
    /// program — it is where the model speaks — so it carries more than the
    /// scaffolding that routes it.
    pub const fn weight(self) -> u32 {
        match self {
            Site::Prompt => 3,
            Site::Mediator => 2,
            Site::Gate => 2,
            Site::Branch => 1,
            Site::Stage => 1,
            Site::Structure => 1,
        }
    }
}

/// One drawing primitive of the composed figure.
///
/// `state` is the cell state the mark takes its colour from — the palette is the
/// caller's, so the same composition can be painted into egui's theme or written
/// out as a flat image without either owning the geometry.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Dot {
        at: (f32, f32),
        radius: f32,
        state: u8,
        alpha: f32,
        filled: bool,
    },
    Line {
        from: (f32, f32),
        to: (f32, f32),
        width: f32,
        state: u8,
        alpha: f32,
    },
    Poly {
        points: Vec<(f32, f32)>,
        state: u8,
        alpha: f32,
        filled: bool,
        width: f32,
    },
    /// A cell a refusal killed.
    Cross {
        at: (f32, f32),
        arm: f32,
        state: u8,
        alpha: f32,
    },
    /// One bit from the prompt or response tape.  The line is deliberately
    /// radial: a one reaches farther from the figure than a zero, so the
    /// binary value is visible as a pulse train rather than as another graph
    /// edge.  `value` and `bit` keep the exact numeric origin available to
    /// non-egui renderers as well.
    Bit {
        from: (f32, f32),
        to: (f32, f32),
        value: u8,
        bit: u8,
        on: bool,
        stream: BinaryStream,
        alpha: f32,
    },
}

/// Where a state falls on the palette ramp.
///
/// Shared rather than duplicated in each renderer, because a palette split is
/// exactly the kind of constant that drifts between two copies and then the
/// offline preview stops telling the truth about the pane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ramp {
    /// Between the background and the faint stop. A low cell sinks into the
    /// ground, which is what gives the figure voids to have structure against —
    /// a ramp that bottoms out at a visible grey paints a solid plate instead.
    Dim(f32),
    /// Between the faint stop and ordinary ink: the working range.
    Quiet(f32),
    /// Between ink and the accent.
    Loud(f32),
}

/// Where the ramp leaves the background.
const FAINT_FROM: f32 = 0.34;

/// Where the accent begins. High on purpose: the disc is thousands of sectors,
/// and an accent that starts at the midpoint washes the whole figure in it. Kept
/// to the top of the range, a hot cell is a signal.
const ACCENT_FROM: f32 = 0.80;

/// A state's place on the ramp.
pub fn ramp(state: u8) -> Ramp {
    let t = state as f32 / 255.0;
    if t < FAINT_FROM {
        Ramp::Dim(t / FAINT_FROM)
    } else if t < ACCENT_FROM {
        Ramp::Quiet((t - FAINT_FROM) / (ACCENT_FROM - FAINT_FROM))
    } else {
        Ramp::Loud((t - ACCENT_FROM) / (1.0 - ACCENT_FROM))
    }
}

/// One cell: a position the program's geometry put there, and a state the run
/// drives.
#[derive(Clone, Debug)]
pub struct Cell {
    pub site: Site,
    /// Nesting depth — the radial coordinate.
    pub depth: usize,
    /// Current state, 0..=255.
    pub state: u8,
    /// The state this cell is being drawn toward, once an answer has landed.
    pub target: u8,
    /// Neighbour indices. Directed: an arrow's neighbour list is one-way.
    pub neighbours: Vec<usize>,
    /// A gate that answered 0 stops evolving and takes its region with it.
    pub dead: bool,
    /// The generation the refusal landed, if it has. The spacetime dims a dead
    /// cell only from here outward — before the gate refused, the cell was alive
    /// and had values, and dimming its whole history would claim otherwise.
    pub died_at: Option<usize>,
    /// Set once this cell's prompt has actually fired in the run.
    pub fired: bool,
}

/// The whole automaton: a lattice built from one program, a rule built from one
/// run's answers.
pub struct Automaton {
    pub cells: Vec<Cell>,
    /// 512-entry totalistic transition table, derived from answer bytes.
    rule: Vec<u8>,
    pub generation: usize,
    /// Every generation still on screen, oldest first. This is the radial axis
    /// of the figure: the rings between the centre and the rim.
    pub history: Vec<Vec<u8>>,
    /// Shannon entropy of the answers seen so far, bits per byte.
    pub entropy: f32,
    pub prompts_seen: usize,
    pub answers_seen: usize,
    /// Bounded tails of the exact byte streams that drive the figure.  The
    /// counts below remain unbounded (apart from `usize`) so the UI can say
    /// when a displayed tape is only a tail of a longer run.
    prompt_tape: Vec<u8>,
    response_tape: Vec<u8>,
    prompt_bytes_seen: usize,
    response_bytes_seen: usize,
    response_histogram: [u64; 256],
}

/// The maximum lattice size. A deeply expanded program can produce thousands of
/// nodes and a wall of cells reads as noise, so the walk stops rather than
/// drawing something illegible.
const MAX_CELLS: usize = 900;

/// How many generations the figure holds — the number of rings between the
/// centre and the rim.
const HISTORY: usize = 48;

/// A model can return a very large answer.  Retaining only a tail keeps an
/// open generation tab bounded while the rule and byte counters still account
/// for the complete response.
const TAPE_BYTES: usize = 512;

/// The halo is a visual instrument, not a second full-resolution transcript.
/// At eight marks per byte this is enough to show a readable pulse train while
/// keeping immediate-mode painting cheap for a long run.
const HALO_BYTES: usize = 48;

impl Automaton {
    /// Build the lattice from a program's geometry.
    pub fn from_program(expr: &Expr) -> Self {
        let mut cells = Vec::new();
        walk(expr, 0, None, &mut cells);
        Self {
            cells,
            // Rule 90-ish default: an XOR-flavoured table, so an unseeded
            // automaton still moves rather than sitting inert. Replaced the
            // moment a run's answers arrive.
            rule: (0..512).map(|i| ((i as u32 * 90) % 256) as u8).collect(),
            generation: 0,
            history: Vec::new(),
            entropy: 0.0,
            prompts_seen: 0,
            answers_seen: 0,
            prompt_tape: Vec::new(),
            response_tape: Vec::new(),
            prompt_bytes_seen: 0,
            response_bytes_seen: 0,
            response_histogram: [0; 256],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Number of prompt bytes received from execution events.
    pub const fn prompt_bytes_seen(&self) -> usize {
        self.prompt_bytes_seen
    }

    /// Number of model-response bytes received from execution events.
    pub const fn response_bytes_seen(&self) -> usize {
        self.response_bytes_seen
    }

    /// A compact, exact binary rendering of the most recent bytes in one
    /// stream.  The ellipsis is a truthful marker that the tape is bounded;
    /// the bytes shown after it are still the real eight-bit values, not a
    /// hash, score, or chart bin.
    pub fn binary_preview(&self, stream: BinaryStream, max_bytes: usize) -> String {
        let tape = match stream {
            BinaryStream::Prompt => &self.prompt_tape,
            BinaryStream::Response => &self.response_tape,
        };
        if tape.is_empty() {
            return "—".to_string();
        }
        let keep = max_bytes.max(1).min(tape.len());
        let omitted = tape.len() > keep;
        let body = tape[tape.len() - keep..]
            .iter()
            .map(|byte| format!("{byte:08b}"))
            .collect::<Vec<_>>()
            .join(" ");
        if omitted {
            format!("… {body}")
        } else {
            body
        }
    }

    fn remember_bytes(tape: &mut Vec<u8>, seen: &mut usize, bytes: &[u8]) {
        *seen = seen.saturating_add(bytes.len());
        tape.extend_from_slice(bytes);
        if tape.len() > TAPE_BYTES {
            let excess = tape.len() - TAPE_BYTES;
            tape.drain(..excess);
        }
    }

    /// Feed a prompt's text. Seeds the next unfired prompt cell.
    ///
    /// The seed is a rolling hash of the bytes, so two different prompts of the
    /// same length seed differently — a length-only seed would make every
    /// prompt in a repetitive program identical.
    pub fn observe_prompt(&mut self, text: &str) {
        Self::remember_bytes(
            &mut self.prompt_tape,
            &mut self.prompt_bytes_seen,
            text.as_bytes(),
        );
        let seed = text
            .bytes()
            .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
        if let Some(cell) = self
            .cells
            .iter_mut()
            .find(|c| c.site == Site::Prompt && !c.fired)
        {
            cell.state = (seed % 256) as u8;
            cell.fired = true;
        }
        self.prompts_seen += 1;
    }

    /// Feed an answer's text. This is what builds the rule.
    ///
    /// The model's output bytes become the transition table and their entropy
    /// becomes the mixing rate. An answer is therefore not merely displayed — it
    /// decides how the whole lattice evolves from here.
    pub fn observe_answer(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        Self::remember_bytes(
            &mut self.response_tape,
            &mut self.response_bytes_seen,
            text.as_bytes(),
        );
        for byte in text.bytes() {
            self.response_histogram[byte as usize] =
                self.response_histogram[byte as usize].saturating_add(1);
        }
        let total = self.response_bytes_seen as f32;
        self.entropy = self
            .response_histogram
            .iter()
            .filter(|count| **count > 0)
            .map(|count| {
                let p = *count as f32 / total;
                -p * p.log2()
            })
            .sum();

        // The rule table IS the model's bytes: entry `i` is the byte the answer
        // had at position `i`, cycling if the answer is shorter than the table.
        //
        // The obvious alternative — index the byte HISTOGRAM by neighbourhood sum
        // — was what this did first, and it was wrong in a way worth recording.
        // Prose uses maybe seventy of the 256 byte values, so the histogram is
        // mostly zeros, so most of the table was zero and every cell collapsed to
        // black within a few generations. Using the byte sequence keeps the
        // model's actual distribution and its actual order.
        //
        // Stretched to the full range, because text lives in a narrow band
        // (roughly 32..126) and an unstretched table would only ever drive the
        // middle of the palette.
        let bytes = text.as_bytes();
        let low = *bytes.iter().min().unwrap_or(&0) as f32;
        let high = *bytes.iter().max().unwrap_or(&255) as f32;
        let span = (high - low).max(1.0);
        for (index, entry) in self.rule.iter_mut().enumerate() {
            let byte = bytes[index % bytes.len()] as f32;
            let stretched = (((byte - low) / span) * 255.0) as u8;
            // Blended, so a long run accumulates a character rather than being
            // overwritten by whichever answer happened to land last.
            *entry = ((*entry as u16 + stretched as u16 * 3) / 4) as u8;
        }

        // The answer also pulls the most recently fired prompt cell toward it.
        let target = text
            .bytes()
            .fold(0u32, |acc, b| acc.wrapping_mul(131).wrapping_add(b as u32));
        if let Some(cell) = self
            .cells
            .iter_mut()
            .rev()
            .find(|c| c.site == Site::Prompt && c.fired)
        {
            cell.target = (target % 256) as u8;
        }
        self.answers_seen += 1;
    }

    /// A gate refused. Its region stops evolving.
    ///
    /// The visible consequence of `std/seam`'s discipline: a mechanical refusal
    /// does not merely return 0, it removes a whole subtree from the run, and
    /// here you watch that happen.
    pub fn observe_refusal(&mut self) {
        let Some(gate) = self
            .cells
            .iter()
            .position(|c| c.site == Site::Gate && !c.dead)
        else {
            return;
        };
        let depth = self.cells[gate].depth;
        let now = self.generation;
        self.cells[gate].dead = true;
        self.cells[gate].died_at = Some(now);
        // Everything deeper, after this gate, until the lattice returns to the
        // gate's own depth.
        for cell in self.cells.iter_mut().skip(gate + 1) {
            if cell.depth <= depth {
                break;
            }
            cell.dead = true;
            cell.died_at = Some(now);
        }
    }

    /// One generation.
    ///
    /// Totalistic: a cell's next state is the rule table indexed by the
    /// weighted sum of its neighbourhood, then eased toward whatever target the
    /// run has set for it. The easing is what keeps this legible — a pure
    /// totalistic rule at byte resolution looks like static.
    pub fn step(&mut self) {
        if self.cells.is_empty() {
            return;
        }
        let previous: Vec<u8> = self.cells.iter().map(|c| c.state).collect();
        // Entropy sets the mixing rate: a varied answer boils, a terse one
        // settles. Clamped so neither extreme freezes or explodes.
        let mix = (self.entropy / 8.0).clamp(0.15, 0.85);

        for (index, cell) in self.cells.iter_mut().enumerate() {
            if cell.dead {
                cell.state = cell.state.saturating_sub(12);
                continue;
            }
            // Outer-totalistic, weighted toward the cell's OWN state.
            //
            // A plain neighbourhood sum was the first version and it produced a
            // figure banded almost purely in time: most cells here have one or
            // two neighbours, so their sums land in the same few buckets and the
            // whole lattice moves together, which throws away the spatial axis.
            // Doubling the cell's own contribution gives each cell its own
            // trajectory through the rule table while the neighbourhood still
            // pulls neighbours together.
            let sum: u32 = cell
                .neighbours
                .iter()
                .map(|n| previous[*n] as u32 * cell.site.weight())
                .sum::<u32>()
                + previous[index] as u32 * 2;
            let ruled = self.rule[(sum % 512) as usize];
            let eased = if cell.target == 0 {
                ruled
            } else {
                // Toward the answer, but through the rule — so the model's
                // words shape the path and not only the destination.
                (((ruled as f32) * (1.0 - mix)) + ((cell.target as f32) * mix)) as u8
            };
            cell.state = eased;
        }

        self.history
            .push(self.cells.iter().map(|c| c.state).collect());
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.generation += 1;
    }

    /// Mean state, for the pane's readout.
    pub fn mean_state(&self) -> f32 {
        if self.cells.is_empty() {
            return 0.0;
        }
        self.cells.iter().map(|c| c.state as f32).sum::<f32>() / self.cells.len() as f32
    }

    /// How many cells a gate has killed.
    pub fn dead_count(&self) -> usize {
        self.cells.iter().filter(|c| c.dead).count()
    }

    /// Feed a run's transcript, starting at `cursor`, and return the new cursor.
    ///
    /// The transcript is the plain stdout of `kaos rebis run`, the same lines the
    /// runs pane lists. Three of its forms carry the numbers this view is made
    /// of:
    ///
    ///   · `event    prompt started · …` — the prompt text, which seeds a cell;
    ///   · `prompt   …` — continuation lines of a routed multiline prompt;
    ///   · `answer   …` — the model's bytes, which build the rule;
    ///   · `event    binary gate selected 0 branch` — a refusal, which kills a
    ///     region.
    ///
    /// An answer spans however many lines the model wrote, and a live run's last
    /// answer block may still be arriving. Feeding half an answer would compute a
    /// histogram over half its bytes and get the rule wrong, so an unterminated
    /// trailing block is left unconsumed and picked up on a later frame once its
    /// terminator lands. That is why this returns a cursor instead of consuming
    /// everything it is given.
    pub fn consume(&mut self, transcript: &[String], cursor: usize) -> usize {
        const PROMPT: &str = "event    prompt started";
        const PROMPT_CONTINUATION: &str = "prompt   ";
        const ANSWER: &str = "answer   ";
        const REFUSED: &str = "event    binary gate selected 0 branch";

        let mut at = cursor;
        let mut answer = String::new();
        let mut in_trace = false;
        // Where the in-progress answer block began, so an unterminated one can
        // be rewound rather than fed short.
        let mut answer_from = None;

        while at < transcript.len() {
            let line = transcript[at].as_str();
            // `kaos rebis run` prints a live event stream and then repeats
            // firings under TRACE for the final report.  The trace is useful
            // to a human, but feeding it here would count every model answer
            // twice and make the automaton depend on the reporter, not the
            // execution.  Detect `firing` as well so a pane opened after the
            // TRACE marker is still safe.
            if line == "TRACE" || line.starts_with("firing   ") {
                if answer_from.take().is_some() {
                    if !answer.trim().eq_ignore_ascii_case("nothing") {
                        self.observe_answer(&answer);
                    }
                    answer.clear();
                }
                in_trace = true;
                at += 1;
                continue;
            }
            if in_trace {
                at += 1;
                continue;
            }
            if let Some(rest) = line.strip_prefix(ANSWER) {
                if answer_from.is_none() {
                    answer_from = Some(at);
                }
                if !answer.is_empty() {
                    answer.push('\n');
                }
                answer.push_str(rest);
                at += 1;
                continue;
            }

            // A non-answer line terminates any block in progress.
            if answer_from.take().is_some() {
                if !answer.trim().eq_ignore_ascii_case("nothing") {
                    self.observe_answer(&answer);
                }
                answer.clear();
            }

            if let Some(rest) = line.strip_prefix(PROMPT) {
                // The head of the prompt is after the last `· `; everything
                // before it is the abstraction metadata.
                let head = rest.rsplit_once("· ").map_or(rest, |(_, head)| head);
                self.observe_prompt(head);
            } else if let Some(rest) = line.strip_prefix(PROMPT_CONTINUATION) {
                // The first event line already contributed the first prompt
                // line. Preserve the line break before each continuation so
                // the tape is the routed prompt's bytes, not a flattened
                // preview of it.
                Self::remember_bytes(&mut self.prompt_tape, &mut self.prompt_bytes_seen, b"\n");
                Self::remember_bytes(
                    &mut self.prompt_tape,
                    &mut self.prompt_bytes_seen,
                    rest.as_bytes(),
                );
            } else if line == REFUSED {
                self.observe_refusal();
            }
            at += 1;
        }

        // The transcript ended mid-answer: rewind to the block's start.
        match answer_from {
            Some(start) => start,
            None => at,
        }
    }

    /// The display order of the cells: grouped by nesting depth, then by the
    /// order the program's own traversal produced them.
    ///
    /// This is the angular axis of the figure, and grouping by depth is what
    /// keeps the program legible in it — each abstraction level occupies one
    /// contiguous arc, so a square's ring really is a ring and not four cells
    /// scattered around the disc.
    pub fn order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.cells.len()).collect();
        order.sort_by_key(|index| (self.cells[*index].depth, *index));
        order
    }

    /// Compose the whole figure as drawing primitives, back to front.
    ///
    /// # The composition
    ///
    /// This is an automaton's own picture — a spacetime diagram — wrapped into a
    /// disc:
    ///
    ///   · **angle is space.** One sector per cell, in [`Self::order`], so each
    ///     nesting depth is a contiguous arc and the program's shape is the
    ///     figure's shape. Radial hairlines mark where one depth ends and the
    ///     next begins.
    ///   · **radius is time.** One ring per generation, oldest at the centre,
    ///     newest at the rim. The figure therefore *grows outward as it
    ///     computes*: what you are looking at is the last fifty generations of
    ///     the lattice at once, not a snapshot of the current one.
    ///   · **colour is the value.** Each sector takes the byte that cell held in
    ///     that generation, and those bytes came out of the rule table, which
    ///     came out of the model's own output.
    ///
    /// So the whole disc is the run's numbers. A settled run bands into smooth
    /// rings; a high-entropy one turns to static; a refusal cuts a dark wedge
    /// from the rim inward, starting at the generation it happened.
    ///
    /// At the rim, one glyph per cell says what part of the program that sector
    /// is — a filled disc for a prompt, a standing square for a mediator, a
    /// hexagon for a gate — so the spacetime can be read against the code.
    ///
    /// Pure geometry: no egui, no colours resolved, nothing but positions and
    /// the byte each mark takes its colour from. That is what lets
    /// `examples/generation_preview.rs` render exactly what the pane draws.
    ///
    /// `extent` is the radius available to the rim.
    pub fn compose(&self, centre: (f32, f32), extent: f32) -> Vec<Mark> {
        use std::f32::consts::TAU;

        let order = self.order();
        if order.is_empty() || extent <= 0.0 {
            return Vec::new();
        }
        let mut marks = Vec::new();

        // Generations, oldest first, with the live states as the newest ring.
        // `history` only fills once the automaton has stepped, so before that the
        // figure is a single ring of seeds — correct, and it shows the lattice
        // exists before it has computed anything.
        let mut frames: Vec<&[u8]> = self.history.iter().map(Vec::as_slice).collect();
        let live: Vec<u8> = self.cells.iter().map(|cell| cell.state).collect();
        frames.push(&live);

        // A large lattice times a full history is tens of thousands of sectors,
        // which no immediate-mode painter should be asked to draw every frame.
        // Thinning generations rather than cells keeps every cell present — the
        // program must never be shown incomplete — and only coarsens time.
        const SECTOR_BUDGET: usize = 12_000;
        // Ceiling, not floor: a floor divide returns 1 for anything under twice
        // the budget, so the thinning never engaged until the figure was already
        // twice too big.
        let stride = (frames.len() * order.len())
            .div_ceil(SECTOR_BUDGET.max(1))
            .max(1);
        let kept: Vec<&&[u8]> = frames
            .iter()
            .rev()
            .step_by(stride)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        // A hole at the centre: the innermost ring would otherwise be a point,
        // and the oldest generation would be squeezed into nothing.
        let hole = extent * 0.16;
        let band = (extent - hole) / kept.len() as f32;
        let sweep = TAU / order.len() as f32;
        let point = |angle: f32, radius: f32| -> (f32, f32) {
            (
                centre.0 + angle.cos() * radius,
                centre.1 + angle.sin() * radius,
            )
        };

        // ── the spacetime ───────────────────────────────────────────────────
        // Which generation each kept ring is, so a refusal dims only the rings
        // from the refusal onward.
        let newest = self.generation;
        let oldest = newest.saturating_sub(frames.len().saturating_sub(1));
        for (ring, states) in kept.iter().enumerate() {
            let when = oldest + (newest - oldest) * ring / kept.len().saturating_sub(1).max(1);
            let inner = hole + band * ring as f32 - 0.7;
            let outer = inner + band + 0.7;
            // The past recedes. The newest ring is full strength so the current
            // generation is unambiguous.
            let age = (kept.len() - 1 - ring) as f32 / kept.len() as f32;
            let alpha = 1.0 - age * 0.62;

            for (slot, cell) in order.iter().enumerate() {
                let Some(state) = states.get(*cell) else {
                    continue;
                };
                // Sectors are grown by a pixel ONLY on the edges facing
                // already-drawn neighbours — inward in time, backward in angle.
                //
                // Antialiased edges that merely touch leave a dark seam, and a
                // grid of seams reads as a lattice drawn over the data. But
                // growing both edges double-blends the overlap and the seam comes
                // back as a BRIGHT line, which is what the first two attempts
                // did. Growing only the trailing edges means the later sector
                // simply wins the boundary pixel, and nothing is composited
                // twice.
                let from = sweep * slot as f32 - 1.0 / extent;
                let to = from + sweep + 1.0 / extent;
                // Subdivide a wide sector's arcs, or a lattice with few cells
                // draws straight-edged pie slices instead of a disc.
                let steps = ((sweep / 0.12).ceil() as usize).clamp(1, 12);
                let mut points = Vec::with_capacity((steps + 1) * 2);
                for step in 0..=steps {
                    let angle = from + (to - from) * step as f32 / steps as f32;
                    points.push(point(angle, inner));
                }
                for step in (0..=steps).rev() {
                    let angle = from + (to - from) * step as f32 / steps as f32;
                    points.push(point(angle, outer));
                }
                marks.push(Mark::Poly {
                    points,
                    state: *state,
                    // A dead cell's sector is drawn faintly rather than dropped,
                    // so the region reads as removed instead of never present.
                    alpha: match self.cells[*cell].died_at {
                        Some(died) if died <= when => alpha * 0.22,
                        _ => alpha,
                    },
                    filled: true,
                    width: 0.0,
                });
            }
        }

        // ── the depth boundaries ────────────────────────────────────────────
        //
        // One hairline wherever the arc crosses into another nesting level. This
        // is the program asserting itself over the spacetime: the sectors between
        // two lines are one abstraction level of the code.
        for (slot, cell) in order.iter().enumerate() {
            let previous = slot
                .checked_sub(1)
                .map(|before| self.cells[order[before]].depth);
            if previous == Some(self.cells[*cell].depth) {
                continue;
            }
            let angle = sweep * slot as f32;
            marks.push(Mark::Line {
                from: point(angle, hole),
                to: point(angle, extent),
                width: 1.0,
                state: 255,
                alpha: 0.20,
            });
        }

        // ── the binary halo ────────────────────────────────────────────────
        //
        // Prompts occupy the left semicircle and responses the right.  Each
        // byte is eight radial pulses, MSB first.  A one reaches farther than
        // a zero; the labels in the pane expose the exact bit strings while
        // this halo lets the eye feel their density as the run evolves.
        let binary_halo =
            |marks: &mut Vec<Mark>, tape: &[u8], stream: BinaryStream, radius: f32| {
                if tape.is_empty() {
                    return;
                }
                let keep = tape.len().min(HALO_BYTES);
                let bytes = &tape[tape.len() - keep..];
                let bit_count = bytes.len() * 8;
                let half_turn = std::f32::consts::PI;
                let start = match stream {
                    BinaryStream::Prompt => std::f32::consts::PI,
                    BinaryStream::Response => 0.0,
                };
                let step = half_turn / bit_count as f32;
                for (byte_index, value) in bytes.iter().copied().enumerate() {
                    for bit in 0..8u8 {
                        let on = value & (1 << (7 - bit)) != 0;
                        let slot = byte_index * 8 + bit as usize;
                        let angle = start + step * (slot as f32 + 0.5);
                        let from = point(angle, radius);
                        let to = point(angle, radius + if on { 9.0 } else { 2.5 });
                        marks.push(Mark::Bit {
                            from,
                            to,
                            value,
                            bit,
                            on,
                            stream,
                            alpha: if on { 0.92 } else { 0.22 },
                        });
                    }
                }
            };
        binary_halo(
            &mut marks,
            &self.prompt_tape,
            BinaryStream::Prompt,
            extent + 1.0,
        );
        binary_halo(
            &mut marks,
            &self.response_tape,
            BinaryStream::Response,
            extent + 7.0,
        );

        // ── the rim ─────────────────────────────────────────────────────────
        //
        // One glyph per cell, so the spacetime can be read against the code.
        //
        // Only when a sector is wide enough to hold one. On a large lattice the
        // sectors are a few pixels across and the glyphs merge into a bead
        // necklace that says nothing — the census in the pane's legend already
        // reports the composition, so the honest move is to draw no glyph rather
        // than an unreadable one.
        let rim = extent + 16.0;
        if sweep * rim < 9.0 {
            return marks;
        }
        for (slot, cell) in order.iter().enumerate() {
            let angle = sweep * (slot as f32 + 0.5);
            let here = point(angle, rim);
            let cell = &self.cells[*cell];
            let heat = cell.state as f32 / 255.0;
            let size = 3.0 + heat * 3.0;

            if cell.dead {
                marks.push(Mark::Cross {
                    at: here,
                    arm: 4.0,
                    state: 200,
                    alpha: 0.75,
                });
                continue;
            }
            match cell.site {
                // A prompt is the only form that fires a model, so it is the only
                // filled disc — and once it has fired it carries a ring.
                Site::Prompt => {
                    marks.push(Mark::Dot {
                        at: here,
                        radius: size + 1.0,
                        state: cell.state,
                        alpha: 1.0,
                        filled: true,
                    });
                    if cell.fired {
                        marks.push(Mark::Dot {
                            at: here,
                            radius: size + 4.0,
                            state: cell.state,
                            alpha: 0.6,
                            filled: false,
                        });
                    }
                }
                // A mediator is a square standing on its point: the `([M] …)`
                // figure itself.
                Site::Mediator => {
                    let r = size + 2.0;
                    marks.push(Mark::Poly {
                        points: vec![
                            (here.0, here.1 - r),
                            (here.0 + r, here.1),
                            (here.0, here.1 + r),
                            (here.0 - r, here.1),
                        ],
                        state: cell.state,
                        alpha: 1.0,
                        filled: false,
                        width: 1.2,
                    });
                }
                // A branch points outward, away from the mediator it hangs off.
                Site::Branch => {
                    let r = size + 1.0;
                    marks.push(Mark::Poly {
                        points: vec![
                            point(angle, rim + r * 1.6),
                            (
                                here.0 + (angle + 2.2).cos() * r,
                                here.1 + (angle + 2.2).sin() * r,
                            ),
                            (
                                here.0 + (angle - 2.2).cos() * r,
                                here.1 + (angle - 2.2).sin() * r,
                            ),
                        ],
                        state: cell.state,
                        alpha: 1.0,
                        filled: true,
                        width: 0.0,
                    });
                }
                // A stage is a bar laid across the flow: an arrow chain is a
                // line, and a stage is a station on it.
                Site::Stage => {
                    let across = angle + std::f32::consts::FRAC_PI_2;
                    marks.push(Mark::Line {
                        from: (here.0 + across.cos() * size, here.1 + across.sin() * size),
                        to: (here.0 - across.cos() * size, here.1 - across.sin() * size),
                        width: 2.0,
                        state: cell.state,
                        alpha: 1.0,
                    });
                }
                // A gate is a hexagon. It is the one form that can remove a
                // region, so nothing else gets this shape.
                Site::Gate => {
                    let r = size + 2.5;
                    marks.push(Mark::Poly {
                        points: (0..6)
                            .map(|i| {
                                let a = TAU * i as f32 / 6.0;
                                (here.0 + a.cos() * r, here.1 + a.sin() * r)
                            })
                            .collect(),
                        state: cell.state,
                        alpha: 1.0,
                        filled: false,
                        width: 1.2,
                    });
                }
                // Scaffolding: present, quiet, small.
                Site::Structure => {
                    marks.push(Mark::Dot {
                        at: here,
                        radius: 1.4,
                        state: cell.state,
                        alpha: 0.5,
                        filled: true,
                    });
                }
            }
        }

        // The centre is time zero. Its ring breathes with the mean state, so the
        // figure has a pulse even when the spacetime has settled into bands.
        let mean = self.mean_state();
        marks.push(Mark::Dot {
            at: centre,
            radius: hole * (0.45 + (mean / 255.0) * 0.35),
            state: mean as u8,
            alpha: 0.8,
            filled: false,
        });

        marks
    }

    /// Cells by site, for the legend.
    pub fn census(&self) -> BTreeMap<&'static str, usize> {
        let mut out = BTreeMap::new();
        for cell in &self.cells {
            let name = match cell.site {
                Site::Prompt => "prompt",
                Site::Mediator => "mediator",
                Site::Branch => "branch",
                Site::Stage => "stage",
                Site::Gate => "gate",
                Site::Structure => "structure",
            };
            *out.entry(name).or_default() += 1;
        }
        out
    }
}

/// Walk the expression, emitting cells and wiring neighbourhoods.
///
/// `parent` is the index a child should attach to. The wiring — not the drawing
/// — is where the language's semantics enter: what becomes adjacent to what is
/// decided by which form we are in.
fn walk(expr: &Expr, depth: usize, parent: Option<usize>, cells: &mut Vec<Cell>) {
    if cells.len() >= MAX_CELLS {
        return;
    }
    let push = |site: Site, cells: &mut Vec<Cell>| -> usize {
        let index = cells.len();
        cells.push(Cell {
            site,
            depth,
            state: 0,
            target: 0,
            neighbours: parent.into_iter().collect(),
            dead: false,
            died_at: None,
            fired: false,
        });
        index
    };

    match expr {
        // A model selector is execution metadata on the same form, not an
        // extra site in the program's geometry.
        Expr::Model { body, .. } => walk(body, depth, parent, cells),
        Expr::Prompt(text) => {
            let me = push(Site::Prompt, cells);
            // A prompt's own bytes give it a resting state even before it fires,
            // so the lattice has structure at generation zero.
            let seed = text
                .bytes()
                .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
            cells[me].state = (seed % 128) as u8;
        }
        Expr::Symbol(_) => {
            push(Site::Structure, cells);
        }
        Expr::Square { mediator, branches } => {
            let centre = push(Site::Mediator, cells);
            walk(mediator, depth + 1, Some(centre), cells);
            // The ring. Each branch attaches to the mediator and to NOTHING
            // ELSE — square branches are mutually isolated in the language, so
            // they are mutually non-adjacent here. That isolation is the whole
            // reason a square's agreement carries information, and drawing them
            // as neighbours would misrepresent it.
            for branch in branches {
                let root = cells.len();
                walk(branch, depth + 1, Some(centre), cells);
                // Mark the branch's own root as a branch — that is what makes
                // the ring a ring. A prompt keeps its own site, because firing a
                // model outranks sitting in a ring: it is the thing that costs.
                if let Some(cell) = cells.get_mut(root) {
                    if cell.site == Site::Structure {
                        cell.site = Site::Branch;
                    }
                }
            }
        }
        Expr::Forward(from, to) => {
            let stage = push(Site::Stage, cells);
            walk(from, depth + 1, Some(stage), cells);
            // Directed: the consumer sees the producer, not the reverse.
            let before = cells.len();
            walk(to, depth + 1, Some(stage), cells);
            if before < cells.len() {
                cells[before].neighbours.push(stage);
            }
        }
        Expr::Backflow(to, from) => {
            let stage = push(Site::Stage, cells);
            // Backflow runs its right operand first, so the wiring is reversed
            // — the same asymmetry `^` inverts.
            walk(from, depth + 1, Some(stage), cells);
            walk(to, depth + 1, Some(stage), cells);
        }
        Expr::Conditional {
            condition,
            when_yes,
            when_no,
        } => {
            let gate = push(Site::Gate, cells);
            walk(condition, depth + 1, Some(gate), cells);
            walk(when_yes, depth + 1, Some(gate), cells);
            walk(when_no, depth + 1, Some(gate), cells);
        }
        Expr::Program(children) | Expr::Compose(children) | Expr::Concat(children) => {
            let node = push(Site::Structure, cells);
            for child in children {
                walk(child, depth + 1, Some(node), cells);
            }
        }
        Expr::Quote(inner) | Expr::Unquote(inner) | Expr::Invert(inner) => {
            let node = push(Site::Structure, cells);
            walk(inner, depth + 1, Some(node), cells);
        }
        Expr::Function { body, .. } => {
            let node = push(Site::Structure, cells);
            walk(body, depth + 1, Some(node), cells);
        }
        Expr::Call { args, .. } => {
            let node = push(Site::Structure, cells);
            for argument in args {
                walk(argument, depth + 1, Some(node), cells);
            }
        }
        // Anything the language adds later gets a structural cell rather than
        // being silently dropped from the lattice.
        _ => {
            push(Site::Structure, cells);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(source: &str) -> Automaton {
        Automaton::from_program(&rebis_lang::parse(source).expect("parse"))
    }

    /// The lattice comes from the program, so different programs give different
    /// lattices — this is the whole premise.
    #[test]
    fn the_lattice_is_built_from_the_programs_geometry() {
        let square = build(r#"(["judge"] "a" "b" "c")"#);
        let chain = build(r#"(-> "a" "b" "c")"#);
        assert!(!square.is_empty() && !chain.is_empty());
        assert_ne!(
            square.census(),
            chain.census(),
            "a square and a chain must not produce the same lattice"
        );
        assert_eq!(square.census().get("mediator").copied(), Some(1));
        assert_eq!(
            square.census().get("prompt").copied(),
            Some(4),
            "3 branches + mediator prompt"
        );
    }

    /// The language's isolation rule, preserved in the wiring. A square's
    /// branches cannot observe each other, so their cells must not be
    /// neighbours — drawing them adjacent would misrepresent why square
    /// agreement carries information.
    #[test]
    fn square_branches_are_not_adjacent_to_each_other() {
        // A symbol mediator, so every depth-1 prompt is a branch and the test
        // does not have to exclude the mediator by position.
        let a = build(r#"([m] "one" "two" "three")"#);
        let branch_indices: Vec<usize> = a
            .cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.site == Site::Prompt && c.depth == 1)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(branch_indices.len(), 3, "three branches in the ring");
        for &i in &branch_indices {
            for &j in &branch_indices {
                if i != j {
                    assert!(
                        !a.cells[i].neighbours.contains(&j),
                        "branch {i} is adjacent to sibling {j}"
                    );
                }
            }
        }
    }

    /// A branch's root is tagged as a branch, so the ring is drawn as a ring —
    /// unless it is a prompt, which keeps its own site because firing a model is
    /// the more important fact about it.
    #[test]
    fn a_branch_root_is_tagged_a_branch_unless_it_fires() {
        let a = build(r#"([m] "a prompt" (-> "x" "y") ([n] "q"))"#);
        let ring: Vec<Site> = a
            .cells
            .iter()
            .filter(|c| c.depth == 1)
            .map(|c| c.site)
            .collect();
        assert!(
            ring.contains(&Site::Prompt),
            "the prompt branch keeps its site: {ring:?}"
        );
        assert!(
            ring.contains(&Site::Stage),
            "the arrow branch keeps its site: {ring:?}"
        );
        assert!(
            ring.contains(&Site::Mediator),
            "the nested square keeps its site: {ring:?}"
        );

        // A group branch is pure scaffolding, so it becomes the ring position.
        let grouped = build(r#"([m] ("one" "two") "three")"#);
        assert!(
            grouped.cells.iter().any(|c| c.site == Site::Branch),
            "a structural branch is tagged as a branch"
        );
    }

    /// A gate that refuses removes its whole region, which is the visible form
    /// of a mechanical refusal.
    #[test]
    fn a_refused_gate_kills_its_region_and_nothing_above_it() {
        // The gate is nested, so there is something above it to survive. A
        // top-level gate legitimately takes everything, since it IS the root.
        let mut a = build(r#"(-> "before" (% (gate "ask") (-> "yes" "more") "no"))"#);
        assert_eq!(a.dead_count(), 0);
        let gate = a
            .cells
            .iter()
            .position(|c| c.site == Site::Gate)
            .expect("the program has a gate");
        assert!(gate > 0, "the gate must not be the root in this fixture");

        a.observe_refusal();
        assert!(a.dead_count() > 1, "the gate took its subtree with it");
        assert!(a.cells[gate].dead, "the gate itself is dead");
        for above in 0..gate {
            assert!(
                !a.cells[above].dead,
                "cell {above} sits above the gate and must survive its refusal"
            );
        }
    }

    /// The rule is derived from the answers, so two different models produce
    /// two different automata from one program. That is the claim this view
    /// makes and it has to be true.
    #[test]
    fn different_answers_produce_different_evolution() {
        let program = r#"(["j"] "a" "b")"#;
        let mut terse = build(program);
        let mut verbose = build(program);
        terse.observe_answer("ok");
        verbose.observe_answer(
            "A long and varied answer, with punctuation, numbers 12345, and \
             considerable lexical range across its bytes.",
        );
        for _ in 0..12 {
            terse.step();
            verbose.step();
        }
        let a: Vec<u8> = terse.cells.iter().map(|c| c.state).collect();
        let b: Vec<u8> = verbose.cells.iter().map(|c| c.state).collect();
        assert_ne!(a, b, "the model's own bytes must shape the automaton");
        assert!(
            verbose.entropy > terse.entropy,
            "a varied answer must read as higher entropy: {} vs {}",
            verbose.entropy,
            terse.entropy
        );
    }

    #[test]
    fn stepping_is_deterministic() {
        let mut a = build(r#"(-> "one" (["m"] "two" "three"))"#);
        let mut b = build(r#"(-> "one" (["m"] "two" "three"))"#);
        for text in ["first answer", "second answer"] {
            a.observe_answer(text);
            b.observe_answer(text);
        }
        for _ in 0..20 {
            a.step();
            b.step();
        }
        assert_eq!(
            a.cells.iter().map(|c| c.state).collect::<Vec<_>>(),
            b.cells.iter().map(|c| c.state).collect::<Vec<_>>()
        );
        assert_eq!(a.generation, 20);
    }

    /// A deeply expanded program must not produce an illegible wall of cells.
    #[test]
    fn the_lattice_is_bounded() {
        let deep = format!("({})", r#"(["m"] "a" "b" "c" "d")"#.repeat(400));
        let a = Automaton::from_program(&rebis_lang::parse(&deep).expect("parse"));
        assert!(a.cells.len() <= MAX_CELLS, "{} cells", a.cells.len());
    }

    #[test]
    fn history_is_kept_bounded_for_the_trace() {
        let mut a = build(r#"(["m"] "a" "b")"#);
        for _ in 0..HISTORY * 2 {
            a.step();
        }
        assert_eq!(a.history.len(), HISTORY);
    }

    /// Prompts seed from their own bytes, so a repetitive program still has
    /// structure at generation zero.
    #[test]
    fn distinct_prompts_seed_distinct_states() {
        let a = build(r#"(["m"] "alpha" "beta")"#);
        let states: Vec<u8> = a
            .cells
            .iter()
            .filter(|c| c.site == Site::Prompt)
            .map(|c| c.state)
            .collect();
        assert!(
            states
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 1,
            "every prompt seeded identically: {states:?}"
        );
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;

    fn machine() -> Automaton {
        Automaton::from_program(
            &rebis_lang::parse(r#"(% (gate "ask") (-> "one" "two") "no")"#).expect("parse"),
        )
    }

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|line| (*line).to_string()).collect()
    }

    /// The three forms that carry numbers are recognised, and nothing else is.
    #[test]
    fn a_transcript_feeds_prompts_answers_and_refusals() {
        let mut a = machine();
        let transcript = lines(&[
            "mode        DRY · deterministic, no provider or tools",
            "event    prompt started · abstraction 2 · read the tape",
            "answer   the tape is flat",
            "event    forward value routed",
            "complete    ✓ run finished",
        ]);

        let cursor = a.consume(&transcript, 0);
        assert_eq!(
            cursor,
            transcript.len(),
            "a finished transcript is consumed"
        );
        assert_eq!(a.prompts_seen, 1);
        assert_eq!(a.answers_seen, 1);
        assert!(a.entropy > 0.0, "the answer's bytes set the mixing rate");
    }

    #[test]
    fn the_binary_tapes_keep_exact_prompt_and_response_bytes() {
        let mut a = machine();
        a.observe_prompt("A");
        a.observe_answer("B");

        assert_eq!(a.prompt_bytes_seen(), 1);
        assert_eq!(a.response_bytes_seen(), 1);
        assert_eq!(a.binary_preview(BinaryStream::Prompt, 1), "01000001");
        assert_eq!(a.binary_preview(BinaryStream::Response, 1), "01000010");
        assert!(a.compose((0.0, 0.0), 100.0).iter().any(|mark| {
            matches!(
                mark,
                Mark::Bit {
                    stream: BinaryStream::Prompt,
                    value: 0x41,
                    bit: 0,
                    on: false,
                    ..
                }
            )
        }));
    }

    #[test]
    fn multiline_prompt_continuations_are_kept_on_the_prompt_tape() {
        let mut a = machine();
        let transcript = lines(&[
            "event    prompt started · abstraction 1 · first line",
            "prompt   second line",
            "answer   response",
            "complete    ✓ run finished",
        ]);
        a.consume(&transcript, 0);

        assert_eq!(
            a.binary_preview(BinaryStream::Prompt, 2),
            "… 01101110 01100101"
        );
    }

    #[test]
    fn the_trace_report_does_not_count_live_answers_again() {
        let mut a = machine();
        let transcript = lines(&[
            "event    prompt started · abstraction 1 · read",
            "answer   one live answer",
            "complete    ✓ run finished",
            "TRACE",
            "firing   1 · ○ prompt read · abstraction 1",
            "answer   one live answer",
            "score    1.000",
        ]);

        assert_eq!(a.consume(&transcript, 0), transcript.len());
        assert_eq!(a.answers_seen, 1);
        assert_eq!(
            a.response_bytes_seen(),
            "one live answer".len(),
            "the final report is a restatement, not another model response"
        );
    }

    #[test]
    fn dry_run_nothing_is_not_mistaken_for_a_model_response() {
        let mut a = machine();
        let transcript = lines(&[
            "event    prompt started · abstraction 1 · dry prompt",
            "answer   nothing",
            "complete    ✓ run finished",
        ]);
        a.consume(&transcript, 0);

        assert_eq!(a.answers_seen, 0);
        assert_eq!(a.response_bytes_seen(), 0);
        assert_eq!(
            a.binary_preview(BinaryStream::Response, 1),
            "—",
            "dry evaluation has no model-response tape"
        );
    }

    /// The prompt head is the text after the metadata, not the metadata.
    #[test]
    fn the_prompt_head_is_extracted_from_the_event_line() {
        let mut quiet = machine();
        let mut loud = machine();
        quiet.consume(
            &lines(&["event    prompt started · abstraction 1 · read the tape"]),
            0,
        );
        loud.consume(
            &lines(&["event    prompt started · abstraction 1 · read the flow"]),
            0,
        );
        let seeded = |a: &Automaton| {
            a.cells
                .iter()
                .find(|c| c.site == Site::Prompt && c.fired)
                .map(|c| c.state)
        };
        assert_ne!(
            seeded(&quiet),
            seeded(&loud),
            "the head must reach the seed — otherwise every prompt in a run is identical"
        );
    }

    /// A live run's trailing answer is still arriving. Feeding it short would
    /// build the rule from a fraction of the model's bytes.
    #[test]
    fn an_unterminated_answer_block_is_left_for_the_next_frame() {
        let mut a = machine();
        let partial = lines(&[
            "event    prompt started · abstraction 1 · read",
            "answer   first half",
        ]);
        let cursor = a.consume(&partial, 0);
        assert_eq!(cursor, 1, "the cursor rewinds to the answer block's start");
        assert_eq!(
            a.answers_seen, 0,
            "no answer is counted while it is partial"
        );

        let mut whole = partial.clone();
        whole.push("answer   second half".to_string());
        whole.push("complete    ✓ run finished".to_string());
        let cursor = a.consume(&whole, cursor);
        assert_eq!(cursor, whole.len());
        assert_eq!(a.answers_seen, 1, "the block lands once, whole");
    }

    /// Consuming in one pass and consuming incrementally must agree, or the view
    /// would depend on frame timing.
    #[test]
    fn incremental_consumption_matches_a_single_pass() {
        let transcript = lines(&[
            "event    prompt started · abstraction 1 · alpha",
            "answer   a long and varied answer",
            "answer   with a second line",
            "event    prompt started · abstraction 2 · beta",
            "answer   terse",
            "complete    ✓ run finished",
        ]);

        let mut whole = machine();
        whole.consume(&transcript, 0);

        let mut piecewise = machine();
        let mut cursor = 0;
        for upto in 1..=transcript.len() {
            cursor = piecewise.consume(&transcript[..upto], cursor);
        }

        assert_eq!(piecewise.prompts_seen, whole.prompts_seen);
        assert_eq!(piecewise.answers_seen, whole.answers_seen);
        assert_eq!(
            piecewise.entropy, whole.entropy,
            "the rule must not depend on how the lines were chunked"
        );
    }

    /// A refusal in the transcript kills the region, so the view shows a gate
    /// closing without needing any extra plumbing.
    #[test]
    fn a_refusal_line_kills_the_region() {
        let mut a = machine();
        assert_eq!(a.dead_count(), 0);
        a.consume(&lines(&["event    binary gate selected 0 branch"]), 0);
        assert!(a.dead_count() > 0, "the gate's region is dead");
    }

    /// A gate that PASSES is not a refusal. Getting this backwards would draw
    /// every successful gate as a kill.
    #[test]
    fn a_passing_gate_kills_nothing() {
        let mut a = machine();
        a.consume(&lines(&["event    binary gate selected 1 branch"]), 0);
        assert_eq!(a.dead_count(), 0);
    }
}

#[cfg(test)]
mod composition_tests {
    use super::*;

    fn machine(source: &str) -> Automaton {
        Automaton::from_program(&rebis_lang::parse(source).expect("parse"))
    }

    /// The angular axis is grouped by nesting depth. Without this a square's
    /// ring is four sectors scattered around the disc and the program stops
    /// being legible in its own picture.
    #[test]
    fn the_display_order_groups_cells_by_depth() {
        let a = machine(r#"([j] "a" (-> "b" "c") ([k] "d" "e"))"#);
        let depths: Vec<usize> = a.order().iter().map(|i| a.cells[*i].depth).collect();
        assert!(
            depths.windows(2).all(|pair| pair[0] <= pair[1]),
            "depths are not contiguous: {depths:?}"
        );
        assert_eq!(depths.len(), a.cells.len(), "every cell gets a sector");
    }

    /// Every cell appears exactly once. A thinned figure would misreport the
    /// program, which is the one thing this view must not do.
    #[test]
    fn the_display_order_is_a_permutation_of_the_cells() {
        let a = machine(r#"([j] "a" (-> "b" (% (gate "g") "y" "n")) "c")"#);
        let mut seen = a.order();
        seen.sort_unstable();
        assert_eq!(seen, (0..a.cells.len()).collect::<Vec<_>>());
    }

    /// Radius is time and the newest generation is at the rim, so the figure
    /// grows outward as it computes.
    #[test]
    fn the_newest_generation_is_the_outermost_ring() {
        let mut a = machine(r#"([j] "a" "b")"#);
        for _ in 0..8 {
            a.step();
        }
        // Force an unmistakable current state, then find how far out it is drawn.
        for cell in &mut a.cells {
            cell.state = 255;
        }
        let marks = a.compose((0.0, 0.0), 400.0);
        let radius_of = |wanted: u8| -> f32 {
            marks
                .iter()
                .filter_map(|mark| match mark {
                    Mark::Poly { points, state, .. } if *state == wanted => Some(
                        points
                            .iter()
                            .map(|(x, y)| (x * x + y * y).sqrt())
                            .fold(0.0f32, f32::max),
                    ),
                    _ => None,
                })
                .fold(0.0f32, f32::max)
        };
        let newest = radius_of(255);
        assert!(
            newest > 300.0,
            "the live generation is at the rim: {newest}"
        );
    }

    /// A refusal dims a region only from the generation it happened. Dimming its
    /// whole history would claim the region was never alive, which is false — the
    /// gate ran for a while and then closed.
    #[test]
    fn a_refusal_leaves_the_generations_before_it_at_full_strength() {
        let mut a = machine(r#"(-> "before" (% (gate "g") "y" "n"))"#);
        for _ in 0..10 {
            a.step();
        }
        a.observe_refusal();
        for _ in 0..10 {
            a.step();
        }

        let dead = a
            .cells
            .iter()
            .position(|cell| cell.dead)
            .expect("something died");
        let died_at = a.cells[dead].died_at.expect("the death is dated");
        assert!(died_at > 0, "the refusal did not happen at generation zero");

        // Sectors are emitted oldest ring first, so the dead cell's own sectors
        // are in chronological order and the early ones must be undimmed.
        let alphas: Vec<f32> = a
            .compose((0.0, 0.0), 400.0)
            .iter()
            .filter_map(|mark| match mark {
                // Identify this cell's sectors by their angular slot: it is the
                // only cell whose alpha ever drops below the age fade.
                Mark::Poly { alpha, .. } => Some(*alpha),
                _ => None,
            })
            .collect();
        let faintest = alphas.iter().copied().fold(f32::MAX, f32::min);
        let strongest = alphas.iter().copied().fold(0.0f32, f32::max);
        assert!(
            faintest < strongest * 0.5,
            "a refusal must dim something: {faintest} vs {strongest}"
        );
        // And the dead cell must still have full-strength sectors from before it
        // died — otherwise its history was rewritten.
        let live_sectors = a
            .compose((0.0, 0.0), 400.0)
            .iter()
            .filter(|mark| matches!(mark, Mark::Poly { alpha, .. } if *alpha > 0.9))
            .count();
        assert!(
            live_sectors > 0,
            "no sector survived at full strength, so the pre-refusal history was dimmed too"
        );
    }

    /// An empty or degenerate figure must not panic or divide by zero — the pane
    /// draws whatever the program gave it.
    #[test]
    fn a_single_cell_program_composes_without_dividing_by_zero() {
        let a = machine(r#""just one prompt""#);
        let marks = a.compose((100.0, 100.0), 80.0);
        assert!(!marks.is_empty());
        for mark in &marks {
            if let Mark::Poly { points, .. } = mark {
                for (x, y) in points {
                    assert!(x.is_finite() && y.is_finite(), "non-finite point");
                }
            }
        }
    }

    /// Zero extent happens for one frame while a pane is being laid out.
    #[test]
    fn no_extent_composes_nothing_rather_than_panicking() {
        assert!(machine(r#"([j] "a" "b")"#)
            .compose((0.0, 0.0), 0.0)
            .is_empty());
    }

    /// On a dense lattice the rim glyphs would merge into an unreadable bead
    /// necklace, so they are dropped rather than drawn illegibly.
    #[test]
    fn rim_glyphs_are_dropped_when_sectors_are_too_narrow_to_hold_one() {
        let wide = machine(r#"([j] "a" "b" "c")"#);
        let glyphs = |a: &Automaton| {
            a.compose((0.0, 0.0), 400.0)
                .iter()
                .filter(|mark| matches!(mark, Mark::Dot { filled: true, .. }))
                .count()
        };
        assert!(glyphs(&wide) > 0, "a small lattice keeps its glyphs");

        // Three hundred prompts in one square: sectors a couple of pixels across.
        let branches = (0..300)
            .map(|i| format!(r#""branch {i}""#))
            .collect::<Vec<_>>()
            .join(" ");
        let dense = machine(&format!("([j] {branches})"));
        assert!(dense.cells.len() > 200);
        assert_eq!(glyphs(&dense), 0, "a dense lattice drops them");
    }

    /// The sector count stays bounded, so a large program cannot ask the painter
    /// for tens of thousands of shapes a frame.
    #[test]
    fn the_sector_count_is_bounded_by_the_budget() {
        let branches = (0..400)
            .map(|i| format!(r#""branch {i}""#))
            .collect::<Vec<_>>()
            .join(" ");
        let mut a = machine(&format!("([j] {branches})"));
        for _ in 0..HISTORY * 2 {
            a.step();
        }
        let sectors = a
            .compose((0.0, 0.0), 400.0)
            .iter()
            .filter(|mark| matches!(mark, Mark::Poly { filled: true, .. }))
            .count();
        assert!(
            sectors <= 16_000,
            "{sectors} sectors is more than a frame should carry"
        );
        // But every cell is still present in the newest ring: time is coarsened,
        // never the program.
        assert!(
            sectors >= a.cells.len(),
            "cells were dropped, not generations"
        );
    }

    /// The accent is reserved for the top of the range. If it started at the
    /// midpoint the whole disc would be washed in it.
    #[test]
    fn the_accent_covers_only_the_top_of_the_range() {
        let loud = (0..=255u8)
            .filter(|state| matches!(ramp(*state), Ramp::Loud(_)))
            .count();
        assert!(
            loud < 60,
            "{loud} of 256 states reach the accent — it stops being a signal"
        );
        // And the bottom of the range sinks into the background, which is what
        // gives the figure voids to have structure against.
        assert!(matches!(ramp(0), Ramp::Dim(t) if t == 0.0));
        assert!(matches!(ramp(255), Ramp::Loud(t) if t > 0.99));
    }
}
