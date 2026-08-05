//! A Rebis program as sound.
//!
//! The claim this module makes is narrow and testable: *a program is already a
//! number, and the number is already music.* Nothing here is decorative. Every
//! quantity a note carries — when it starts, how long it lasts, what pitch it
//! is, how bright it is — is read off the program with one rule each, and the
//! rules are the ones the language already uses to mean things.
//!
//! **Time is indentation.** The whole piece is one span. A form that holds
//! other forms divides its span among them; a form that holds nothing sounds
//! for the whole of the slice it was given. So the rhythm is the drawing: an
//! indentation is not a pause before the contents, it is the room the contents
//! have to be in. Nesting deeper does not delay a form, it makes it brief —
//! which is exactly what indentation does to a line on the page.
//!
//! **A parallel form is a chord.** `([m] a b …)` runs its branches
//! independently, so they sound at once and the mediator answers after them.
//! The one place the language is genuinely simultaneous is the one place the
//! music is polyphonic; everything else is a single voice moving in time.
//!
//! **Every character is a number, and every number is Fibonacci.** A word's
//! value is its characters read in a positional system whose place values are
//! the Fibonacci numbers — so the same letters in a different order are a
//! different number, and word order is audible. That value is then decomposed
//! by Zeckendorf's theorem, which says every positive integer is a sum of
//! non-consecutive Fibonacci numbers in exactly one way. That decomposition is
//! the whole of the tone: the indices choose the pitch, and each term becomes
//! one partial in the timbre.
//!
//! **Minimal becomes full.** A single character decomposes into one or two
//! terms and sounds as very nearly a sine. A word blooms into a stack of
//! partials on the harmonic numbers 1, 2, 3, 5, 8, 13 — Fibonacci again, and
//! all but the last of them genuine overtones, so the growth is in richness
//! rather than in dissonance. The atom is quiet and pure; what is built out of
//! atoms is bright.
//!
//! The scale is Fibonacci too: the first Fibonacci numbers folded into one
//! octave, which lands on a just-intoned twelve-tone set (`1/1`, `17/16`,
//! `9/8`, `5/4`, `21/16`, `3/2`, `13/8`, `55/32`, …) rather than on anything
//! tempered. See [`SCALE`].
//!
//! Everything above is pure and deterministic: the same source is the same
//! samples, on any machine, with no clock and no randomness anywhere. Only
//! [`sink`] leaves the process, and only to hand a finished file to whatever
//! the operating system uses to play one.

use std::f64::consts::TAU;

use rebis_lang::Expr;

/// How many Fibonacci place values a word is read with, and the size of the
/// table everything else indexes into.
///
/// Sixty terms overflow nothing (`fib(60)` is about `1.5e12`, and a word's
/// value is a sum of those times a character code, which stays inside `u64`
/// for any word a human types) and reach far enough that no realistic word
/// runs out of places.
const PLACES: usize = 60;

/// The Fibonacci numbers, from `fib(1) = 1`.
///
/// Built once, iteratively, because the recursive definition is the one thing
/// about this sequence that is famously not how you compute it.
#[must_use]
pub fn fibonacci() -> [u64; PLACES] {
    let mut table = [0u64; PLACES];
    let (mut a, mut b) = (1u64, 1u64);
    for slot in &mut table {
        *slot = a;
        (a, b) = (b, a.saturating_add(b));
    }
    table
}

/// The twelve pitches of the Fibonacci scale, as ratios in `[1, 2)`.
///
/// Each is a Fibonacci number folded into a single octave by halving it until
/// it lands there — `3 → 3/2`, `5 → 5/4`, `13 → 13/8`, `21 → 21/16` — and then
/// the set is sorted. What comes out is not a tempered scale and is not meant
/// to be: it is a just-intoned twelve where every interval is a ratio of a
/// Fibonacci number to a power of two, so the scale is made of the same
/// arithmetic as the notes played on it.
#[must_use]
pub fn scale() -> Vec<f64> {
    let mut degrees: Vec<f64> = fibonacci()
        .iter()
        .take(16)
        .map(|term| octave_reduce(*term as f64))
        .collect();
    degrees.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    degrees.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    degrees.truncate(SCALE_DEGREES);
    degrees
}

/// How many degrees the scale is cut to. Twelve, because the octave-reduced
/// Fibonacci numbers supply that many distinct ones before they begin to crowd
/// each other closer than a comma.
pub const SCALE_DEGREES: usize = 12;

/// Fold a ratio into the octave above unity.
#[must_use]
pub fn octave_reduce(mut ratio: f64) -> f64 {
    if !ratio.is_finite() || ratio <= 0.0 {
        return 1.0;
    }
    while ratio >= 2.0 {
        ratio *= 0.5;
    }
    while ratio < 1.0 {
        ratio *= 2.0;
    }
    ratio
}

/// The harmonic numbers a tone's partials may land on.
///
/// Fibonacci, and — bar the last — the low harmonic series: doubling, the
/// twelfth, the major third two octaves up, the sixth three octaves up. A tone
/// built from these is a bright natural tone rather than a bell, which is what
/// makes a long word sound like more of the same thing rather than like a
/// different instrument.
const HARMONICS: [f64; 6] = [1.0, 2.0, 3.0, 5.0, 8.0, 13.0];

/// Read a word as a number in the Fibonacci positional system.
///
/// Place `i` is worth `fib(i + 2)`, and the digit there is the character's own
/// code point. Two properties earn this over a hash: it is *positional*, so
/// `abc` and `cba` are different numbers and word order is audible; and it is
/// monotone in length, so a longer word is a larger number and therefore — via
/// Zeckendorf — a richer tone.
///
/// Saturating throughout: a pathological word cannot panic the synth, it only
/// reaches the top of the range and stays there.
#[must_use]
pub fn word_number(word: &str) -> u64 {
    let table = fibonacci();
    word.chars()
        .take(PLACES - 2)
        .enumerate()
        .fold(1u64, |value, (place, character)| {
            let weight = table[place + 1];
            value.saturating_add(weight.saturating_mul(u64::from(character as u32)))
        })
}

/// Zeckendorf's decomposition: the indices of the non-consecutive Fibonacci
/// numbers that sum to `value`, largest first.
///
/// The representation is unique, which is what lets it stand in for the number
/// itself. Greedy is correct here — subtracting the largest Fibonacci number
/// that fits can never leave a remainder needing its neighbour — and that is
/// also why the indices returned are never adjacent.
#[must_use]
pub fn zeckendorf(mut value: u64) -> Vec<usize> {
    let table = fibonacci();
    let mut indices = Vec::new();
    // The search starts at the top of the table and walks down, so of the two
    // Fibonacci numbers that are both 1 it always takes the higher index. That
    // is what stops one number from having two decompositions differing only
    // in which of the ones it used.
    let mut index = table
        .iter()
        .enumerate()
        .rev()
        .find(|(_, term)| **term <= value)
        .map_or(0, |(index, _)| index);
    while value > 0 {
        let term = table[index];
        if term <= value {
            value -= term;
            indices.push(index);
            // Non-consecutive by construction.
            index = index.saturating_sub(2);
        } else if index == 0 {
            break;
        } else {
            index -= 1;
        }
    }
    indices
}

/// The shape of one cycle of a voice's carrier.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Timbre {
    /// One sine per partial. The plainest reading, and the one that lets the
    /// Zeckendorf stack be heard for what it is.
    #[default]
    Sine,
    /// Odd harmonics only, at `1/n²` — soft, hollow, close to a stopped pipe.
    Triangle,
    /// Every harmonic at `1/n` — the reedy one.
    Saw,
    /// Odd harmonics at `1/n` — hollow and loud.
    Square,
}

impl Timbre {
    /// The label used by both front ends, and the name the settings round-trip
    /// through.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Timbre::Sine => "sine",
            Timbre::Triangle => "triangle",
            Timbre::Saw => "saw",
            Timbre::Square => "square",
        }
    }

    /// Every timbre, in the order the front ends cycle through them.
    pub const ALL: [Timbre; 4] = [Timbre::Sine, Timbre::Triangle, Timbre::Saw, Timbre::Square];

    /// The next one, so a single key or button can cycle the synth.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Timbre::Sine => Timbre::Triangle,
            Timbre::Triangle => Timbre::Saw,
            Timbre::Saw => Timbre::Square,
            Timbre::Square => Timbre::Sine,
        }
    }

    /// One cycle of this shape, from a phase in turns.
    fn wave(self, phase: f64) -> f64 {
        let turns = phase.rem_euclid(1.0);
        match self {
            Timbre::Sine => (turns * TAU).sin(),
            Timbre::Triangle => 4.0 * (turns - (turns + 0.5).floor()).abs() - 1.0,
            Timbre::Saw => 2.0 * turns - 1.0,
            Timbre::Square => {
                if turns < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

/// Everything the listener chooses. The program supplies the rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// The frequency the first degree of the scale sits on.
    pub root_hz: f64,
    /// Seconds one atom is given. The piece's length is this times the number
    /// of atoms in the program, so a longer program is a longer piece rather
    /// than the same piece played faster.
    pub pace: f64,
    /// How many Zeckendorf terms are allowed to sound as partials. One is a
    /// sine; six is the whole harmonic set.
    pub partials: usize,
    /// The carrier shape under those partials.
    pub timbre: Timbre,
    /// How many octaves of indentation are used before depth wraps. Keeps a
    /// deeply nested program inside the range of a loudspeaker and of an ear.
    pub octaves: usize,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            // A3. Low enough that four octaves of indentation stay musical,
            // high enough that a single note on a laptop speaker is audible.
            root_hz: 220.0,
            pace: 0.22,
            partials: 4,
            timbre: Timbre::Sine,
            octaves: 4,
        }
    }
}

/// One sounded form.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// Seconds from the start of the piece.
    pub start: f64,
    /// Seconds it is held, before the release tail.
    pub duration: f64,
    /// The fundamental.
    pub hz: f64,
    /// Peak amplitude before mixing.
    pub gain: f64,
    /// Its partials, as multiples of the fundamental with their own gains.
    pub partials: Vec<(f64, f64)>,
    /// How many boundaries this form is written inside.
    pub depth: usize,
    /// What sounded — the word, the sigil, the name. Shown beside the wave so
    /// a listener can read what they are hearing.
    pub token: String,
}

impl Note {
    /// The scale degree this note landed on, for display.
    #[must_use]
    pub fn degree(&self, tuning: &Tuning) -> usize {
        let scale = scale();
        let ratio = octave_reduce(self.hz / tuning.root_hz.max(f64::EPSILON));
        scale
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (a.1 - ratio)
                    .abs()
                    .partial_cmp(&(b.1 - ratio).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(0, |(index, _)| index)
    }
}

/// A whole program, as notes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    pub notes: Vec<Note>,
    /// Seconds from the first onset to the last release.
    pub seconds: f64,
    /// How deep the program is nested — the piece's range, in octaves.
    pub depth: usize,
}

/// How long a note rings past its slot, as a fraction of the slot.
///
/// Without it every note is gated at the boundary and the piece reads as a
/// metronome with pitches on it. A fifth of a slot is enough to hear one form
/// arrive over the last of the one before, which is what makes an indentation
/// sound like a phrase rather than a list.
const RELEASE: f64 = 0.2;

/// The shortest slot a form may be given.
///
/// A deeply nested program divides its span very finely, and past a certain
/// point a note is a click rather than a pitch. Sixteen milliseconds is about
/// where an ear stops hearing an event and starts hearing a texture, which is
/// the right thing for a program with a thousand atoms in it to sound like.
const FLOOR: f64 = 0.016;

impl Score {
    /// Derive a score from Rebis source.
    ///
    /// # Errors
    ///
    /// Returns the parser's own message when the source is not a program. The
    /// music is a reading of the syntax tree, so there is nothing to play
    /// until there is a tree.
    pub fn from_source(source: &str, tuning: &Tuning) -> Result<Self, String> {
        let expr = rebis_lang::parse(source).map_err(|error| error.to_string())?;
        Ok(Self::from_expr(&expr, tuning))
    }

    /// Derive a score from a parsed program.
    #[must_use]
    pub fn from_expr(expr: &Expr, tuning: &Tuning) -> Self {
        let span = (atoms(expr) as f64) * tuning.pace.max(FLOOR);
        let mut notes = Vec::new();
        voice(expr, 0.0, span, 0, tuning, &mut notes);
        let seconds = notes
            .iter()
            .map(|note| note.start + note.duration * (1.0 + RELEASE))
            .fold(0.0_f64, f64::max);
        let depth = notes.iter().map(|note| note.depth).max().unwrap_or(0);
        Self {
            notes,
            seconds,
            depth,
        }
    }

    /// The most voices sounding at once — how thick the program is, and the
    /// number the mix is normalised against.
    ///
    /// Measured on the held slot rather than the release tail. The tail is
    /// deliberately allowed to run into the next form — that overlap is what
    /// makes a level sound like a phrase — and counting it would report every
    /// single-voice program as polyphonic.
    #[must_use]
    pub fn polyphony(&self) -> usize {
        let mut edges: Vec<(f64, i32)> = Vec::with_capacity(self.notes.len() * 2);
        for note in &self.notes {
            edges.push((note.start, 1));
            edges.push((note.start + note.duration, -1));
        }
        // A form ending exactly where the next begins is a handover, not a
        // chord, so an ending is ordered before a beginning at the same instant.
        edges.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        let (mut now, mut most) = (0, 0);
        for (_, delta) in edges {
            now += delta;
            most = most.max(now);
        }
        most.max(1) as usize
    }
}

/// How many atoms — words, names, sigils — a form contains.
///
/// The mass a span is divided by. A prompt is worth its words, so a paragraph
/// takes longer than a symbol beside it, which is the honest reading: it *is*
/// more of the program.
fn atoms(expr: &Expr) -> usize {
    match expr {
        Expr::Prompt(text) => words(text).len().max(1),
        Expr::Source | Expr::Symbol(_) | Expr::Import { .. } => 1,
        Expr::Model { body, .. }
        | Expr::Quote(body)
        | Expr::Unquote(body)
        | Expr::Invert(body)
        | Expr::Dream(body) => 1 + atoms(body),
        Expr::Program(forms)
        | Expr::Compose(forms)
        | Expr::Imaginary(forms)
        | Expr::Concat(forms)
        | Expr::Meta(forms)
        | Expr::Numeric(forms)
        | Expr::Flashback(forms) => 1 + forms.iter().map(atoms).sum::<usize>(),
        Expr::Square { mediator, branches } => {
            1 + atoms(mediator) + branches.iter().map(atoms).sum::<usize>()
        }
        Expr::Conditional {
            condition,
            when_yes,
            when_no,
        } => 1 + atoms(condition) + atoms(when_yes) + atoms(when_no),
        Expr::Forward(from, to) | Expr::Backflow(from, to) => 1 + atoms(from) + atoms(to),
        Expr::Function { body, params, .. } => 1 + params.len() + atoms(body),
        Expr::Call { args, .. } => 1 + args.iter().map(atoms).sum::<usize>(),
        Expr::Ask => 1,
        Expr::Load(path) => 1 + atoms(path),
        Expr::Context { context, body } => 1 + atoms(context) + atoms(body),
        Expr::Supersede { topic, body } => 1 + atoms(topic) + atoms(body),
        Expr::Invariant { check, body } => 1 + atoms(check) + atoms(body),
        Expr::Bind { value, body, .. } => 1 + atoms(value) + atoms(body),
    }
}

/// A prompt's words. Punctuation is a boundary and not a word of its own, so a
/// sentence sounds as its words rather than as its commas.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// The sigil a form is written with — what the form itself sounds as, before
/// its contents do.
fn mark(expr: &Expr) -> String {
    match expr {
        Expr::Program(_) => "program".into(),
        Expr::Source => "<>".into(),
        Expr::Supersede { .. } => "*".into(),
        Expr::Invariant { .. } => "@".into(),
        Expr::Meta(_) => "><".into(),
        Expr::Numeric(_) => "|".into(),
        Expr::Compose(_) => "(".into(),
        Expr::Imaginary(_) => "{".into(),
        Expr::Concat(_) => "$".into(),
        Expr::Flashback(_) => "?".into(),
        Expr::Square { .. } => "[]".into(),
        Expr::Conditional { .. } => "%".into(),
        Expr::Forward(..) => "->".into(),
        Expr::Backflow(..) => "<-".into(),
        Expr::Function { name, .. } => format!("~{name}"),
        Expr::Call { name, .. } => name.clone(),
        Expr::Ask => "&".into(),
        Expr::Load(_) => "&:".into(),
        Expr::Context { .. } => "+".into(),
        Expr::Import { module } => format!("#{module}"),
        Expr::Model { selector, .. } => format!("/{selector}"),
        Expr::Quote(_) => "'".into(),
        Expr::Unquote(_) => ",".into(),
        Expr::Invert(_) => "^".into(),
        Expr::Dream(_) => "!".into(),
        Expr::Bind { name, .. } => format!("={name}"),
        Expr::Prompt(text) => text.clone(),
        Expr::Symbol(name) => name.clone(),
    }
}

/// Sound one form inside the span it was given.
///
/// The rule, whole: **a form's contents divide the form's time, in equal
/// parts.** Not in proportion to how much each holds — equally, because that
/// is what an indentation does. Ten forms in one boundary each get a tenth of
/// it however large or small they are, so writing something one level deeper
/// makes it briefer and nothing else does. Duration is indentation, and the
/// two are the same measurement read twice.
///
/// The form's own sigil takes a short grace slot first — an indentation
/// announces itself before what it holds — of a golden fraction of its span,
/// but never longer than one atom's pace. Capping it is what keeps the outer
/// boundaries of a large program from becoming minute-long drones while their
/// contents flicker underneath.
fn voice(expr: &Expr, start: f64, span: f64, depth: usize, tuning: &Tuning, out: &mut Vec<Note>) {
    let span = span.max(FLOOR);
    match expr {
        Expr::Prompt(text) => phrase(text, start, span, depth, tuning, out),
        Expr::Symbol(_) | Expr::Import { .. } => {
            out.push(tone(&mark(expr), start, span, depth, tuning));
        }
        // A parallel form is the one place the language is genuinely
        // simultaneous, so it is the one place the music is a chord: every
        // branch starts together in the same span, and the mediator answers
        // after them.
        Expr::Square { mediator, branches } => {
            let head = grace(span, tuning);
            out.push(tone(&mark(expr), start, head, depth, tuning));
            let rest = span - head;
            // The branches take the golden share of what is left and the
            // mediator the remainder, so the answer is heard as an answer
            // rather than as one more branch.
            let chord = rest / PHI;
            for branch in branches {
                voice(branch, start + head, chord, depth + 1, tuning, out);
            }
            voice(
                mediator,
                start + head + chord,
                rest - chord,
                depth + 1,
                tuning,
                out,
            );
        }
        _ => {
            let contents = contents(expr);
            if contents.is_empty() {
                out.push(tone(&mark(expr), start, span, depth, tuning));
                return;
            }
            let head = grace(span, tuning);
            out.push(tone(&mark(expr), start, head, depth, tuning));
            let slice = ((span - head) / contents.len() as f64).max(FLOOR);
            for (index, form) in contents.iter().enumerate() {
                voice(
                    form,
                    start + head + slice * index as f64,
                    slice,
                    depth + 1,
                    tuning,
                    out,
                );
            }
        }
    }
}

/// The golden ratio. The scale converges on it and the rhythm divides by it,
/// which is the same fact about the same sequence stated twice.
const PHI: f64 = 1.618_033_988_749_895;

/// How long a boundary's own sigil sounds before its contents do.
///
/// A golden minor share of the span — `1/φ²`, the part that leaves the rest in
/// the same proportion to the whole — held to one atom's pace at the top so
/// that a large program opens with a stroke rather than a drone, and to the
/// floor at the bottom so it is never a click.
fn grace(span: f64, tuning: &Tuning) -> f64 {
    (span / (PHI * PHI))
        .min(tuning.pace.max(FLOOR))
        .clamp(FLOOR.min(span), span * 0.5)
}

/// The forms written inside this one, in written order.
fn contents(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::Program(forms)
        | Expr::Compose(forms)
        | Expr::Imaginary(forms)
        | Expr::Concat(forms)
        | Expr::Meta(forms)
        | Expr::Numeric(forms)
        | Expr::Flashback(forms) => forms.iter().collect(),
        Expr::Model { body, .. }
        | Expr::Quote(body)
        | Expr::Unquote(body)
        | Expr::Invert(body)
        | Expr::Dream(body)
        | Expr::Function { body, .. } => vec![body.as_ref()],
        Expr::Source | Expr::Ask => Vec::new(),
        Expr::Load(path) => vec![path.as_ref()],
        Expr::Context { context, body } => vec![context.as_ref(), body.as_ref()],
        Expr::Supersede { topic, body } => vec![topic.as_ref(), body.as_ref()],
        Expr::Invariant { check, body } => vec![check.as_ref(), body.as_ref()],
        Expr::Bind { value, body, .. } => vec![value.as_ref(), body.as_ref()],
        Expr::Conditional {
            condition,
            when_yes,
            when_no,
        } => vec![condition.as_ref(), when_yes.as_ref(), when_no.as_ref()],
        Expr::Forward(from, to) | Expr::Backflow(from, to) => vec![from.as_ref(), to.as_ref()],
        Expr::Call { args, .. } => args.iter().collect(),
        Expr::Square { mediator, branches } => {
            let mut forms = vec![mediator.as_ref()];
            forms.extend(branches.iter());
            forms
        }
        Expr::Prompt(_) | Expr::Symbol(_) | Expr::Import { .. } => Vec::new(),
    }
}

/// A prompt as a melody: one note per word, in the order they are written.
///
/// This is where the atomic level is actually heard. A prompt is not one event
/// with a long duration — it is its words, and each of them is a number with
/// its own decomposition, so a sentence has a contour that belongs to that
/// sentence and to no other.
fn phrase(text: &str, start: f64, span: f64, depth: usize, tuning: &Tuning, out: &mut Vec<Note>) {
    let words = words(text);
    if words.is_empty() {
        out.push(tone("\"\"", start, span, depth, tuning));
        return;
    }
    let each = (span / words.len() as f64).max(FLOOR);
    for (index, word) in words.iter().enumerate() {
        out.push(tone(word, start + each * index as f64, each, depth, tuning));
    }
}

/// One token as one tone.
///
/// The whole mapping, in one place: the word becomes a number, the number
/// becomes its Zeckendorf terms, the terms choose the degree of the scale and
/// then become the partials above it, and the indentation chooses the octave.
fn tone(token: &str, start: f64, duration: f64, depth: usize, tuning: &Tuning) -> Note {
    let terms = zeckendorf(word_number(token));
    let scale = scale();
    let degree = terms.iter().sum::<usize>() % scale.len();
    let octave = if tuning.octaves == 0 {
        0
    } else {
        depth % tuning.octaves
    };
    let hz = tuning.root_hz.max(1.0) * scale[degree] * 2f64.powi(octave as i32);
    // One partial per Zeckendorf term, on the Fibonacci harmonic numbers, at
    // `1/h`: the further up the harmonic, the quieter, so a long word gains
    // brightness without gaining loudness.
    let mut partials: Vec<(f64, f64)> = Vec::new();
    for index in terms.iter().take(tuning.partials.clamp(1, 16)) {
        let harmonic = HARMONICS[index % HARMONICS.len()];
        if partials.iter().any(|(h, _)| (*h - harmonic).abs() < 1e-9) {
            continue;
        }
        partials.push((harmonic, 1.0 / harmonic));
    }
    if partials.is_empty() {
        partials.push((1.0, 1.0));
    }
    // Normalised so a rich tone and a bare one are the same loudness — the
    // expansion is meant to be heard as colour, not as volume.
    let sum: f64 = partials.iter().map(|(_, gain)| gain).sum();
    for partial in &mut partials {
        partial.1 /= sum.max(f64::EPSILON);
    }
    Note {
        start,
        duration,
        hz,
        // Deeper is quieter, gently: the structure recedes as it subdivides,
        // which is what keeps a thousand-atom program from being a wall.
        gain: 0.9 / (1.0 + depth as f64 * 0.12),
        partials,
        depth,
        token: token.to_string(),
    }
}

/// Samples per second everything is rendered at.
pub const RATE: u32 = 44_100;

/// Render a score to mono samples in `[-1, 1]`.
///
/// Additive and exact: every note is summed at every sample it touches, then
/// the whole is divided by the thickest moment in the piece so that a chord
/// cannot clip while a single line stays quiet. A soft clip catches whatever
/// the division does not.
#[must_use]
pub fn render(score: &Score, tuning: &Tuning) -> Vec<f32> {
    let rate = f64::from(RATE);
    let length = ((score.seconds + 0.05) * rate).ceil().max(1.0) as usize;
    let mut samples = vec![0f64; length];
    for note in &score.notes {
        let tail = note.duration * (1.0 + RELEASE);
        let first = (note.start * rate).max(0.0) as usize;
        let last = (((note.start + tail) * rate).ceil() as usize).min(length);
        for (index, sample) in samples.iter_mut().enumerate().take(last).skip(first) {
            let time = index as f64 / rate - note.start;
            let shape = envelope(time, note.duration);
            if shape <= 0.0 {
                continue;
            }
            let mut value = 0.0;
            for (harmonic, gain) in &note.partials {
                value += gain * tuning.timbre.wave(note.hz * harmonic * time);
            }
            *sample += value * shape * note.gain;
        }
    }
    let headroom = (score.polyphony() as f64).sqrt().max(1.0);
    samples
        .into_iter()
        .map(|sample| (sample / headroom).clamp(-1.0, 1.0).tanh() as f32)
        .collect()
}

/// The amplitude of a note `time` seconds after its onset.
///
/// A short attack so an onset is a note and not a click, a slow decay across
/// the slot, and a release tail that runs past it. Percussive, because every
/// note here is a form arriving rather than a key being held.
fn envelope(time: f64, duration: f64) -> f64 {
    if time < 0.0 {
        return 0.0;
    }
    let attack = (duration * 0.08).clamp(0.002, 0.02);
    let release = duration * RELEASE;
    if time < attack {
        return time / attack;
    }
    if time <= duration {
        // Down to a third across the slot: audibly a decay, never silence
        // before the next form.
        let held = (time - attack) / (duration - attack).max(f64::EPSILON);
        return 1.0 - held * 0.66;
    }
    let out = (time - duration) / release.max(f64::EPSILON);
    (0.34 * (1.0 - out)).max(0.0)
}

/// One column of a drawn waveform: the extremes the samples reach there, and
/// how loud they are on average.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Band {
    pub low: f32,
    pub high: f32,
    pub rms: f32,
}

/// Reduce samples to `columns` bands, for drawing.
///
/// Min and max rather than a sampled point: a waveform drawn by picking one
/// sample per column is an alias pattern, not a wave. Both front ends draw
/// from this, so the terminal and the window show the same figure.
#[must_use]
pub fn waveform(samples: &[f32], columns: usize) -> Vec<Band> {
    if columns == 0 {
        return Vec::new();
    }
    if samples.is_empty() {
        return vec![Band::default(); columns];
    }
    (0..columns)
        .map(|column| {
            let first = samples.len() * column / columns;
            let last = (samples.len() * (column + 1) / columns).max(first + 1);
            let slice = &samples[first.min(samples.len() - 1)..last.min(samples.len())];
            let mut band = Band {
                low: f32::MAX,
                high: f32::MIN,
                rms: 0.0,
            };
            let mut energy = 0.0f64;
            for sample in slice {
                band.low = band.low.min(*sample);
                band.high = band.high.max(*sample);
                energy += f64::from(*sample) * f64::from(*sample);
            }
            if slice.is_empty() {
                return Band::default();
            }
            band.rms = (energy / slice.len() as f64).sqrt() as f32;
            band
        })
        .collect()
}

/// Draw a wave in braille.
///
/// Two dot columns and four dot rows per character, so a terminal panel draws
/// the same figure the window does at eight times the resolution its cells
/// would otherwise allow. Pass `2 × width` bands for a panel `width` columns
/// wide. Each column is filled from the wave's low to its high, so what is
/// drawn is the body of the wave and not a line through the middle of it.
///
/// It lives here beside the synth rather than in the terminal app because it
/// is a pure function from bands to text, and because the alternative was for
/// the two front ends to disagree about the shape of the same file.
#[must_use]
pub fn braille(bands: &[Band], rows: usize) -> Vec<String> {
    // The dot values inside one braille cell, by (column, row).
    const DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    if rows == 0 || bands.is_empty() {
        return Vec::new();
    }
    let height = rows * 4;
    let columns = bands.len().div_ceil(2);
    let mut cells = vec![0u8; columns * rows];
    let dot_row = |amplitude: f32| -> usize {
        let placed = (0.5 - f64::from(amplitude) * 0.5) * (height - 1) as f64;
        (placed.round().max(0.0) as usize).min(height - 1)
    };
    for (index, band) in bands.iter().enumerate() {
        let (column, half) = (index / 2, index % 2);
        if column >= columns {
            break;
        }
        // Never empty: a silent column still draws the axis, so the piece has
        // a line to be read against.
        let top = dot_row(band.high.max(0.0));
        let bottom = dot_row(band.low.min(0.0));
        for row in top..=bottom {
            cells[(row / 4) * columns + column] |= DOTS[half][row % 4];
        }
    }
    (0..rows)
        .map(|row| {
            cells[row * columns..(row + 1) * columns]
                .iter()
                .map(|cell| char::from_u32(0x2800 + u32::from(*cell)).unwrap_or(' '))
                .collect()
        })
        .collect()
}

/// Encode mono samples as a 16-bit PCM WAV file.
///
/// Written by hand because it is forty-four bytes of header and a cast, and
/// because a dependency for that would be the only dependency in this crate
/// that was not the language itself.
#[must_use]
pub fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    let mut put = |bytes: &[u8]| out.extend_from_slice(bytes);
    put(b"RIFF");
    put(&((36 + data_len) as u32).to_le_bytes());
    put(b"WAVEfmt ");
    put(&16u32.to_le_bytes()); // PCM header length
    put(&1u16.to_le_bytes()); // uncompressed
    put(&1u16.to_le_bytes()); // mono
    put(&RATE.to_le_bytes());
    put(&(RATE * 2).to_le_bytes()); // bytes per second
    put(&2u16.to_le_bytes()); // bytes per frame
    put(&16u16.to_le_bytes()); // bits per sample
    put(b"data");
    put(&(data_len as u32).to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        put(&value.to_le_bytes());
    }
    out
}

/// Handing a finished file to the operating system.
///
/// The one part of this module that leaves the process. It lives in the core
/// rather than in either front end because the ladder of players is a fact
/// about the machine, not about the terminal or the window, and writing it
/// twice would mean two lists to keep in step. Nothing here synthesises,
/// draws, or decides anything musical.
pub mod sink {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};

    /// The players tried, in order, with the arguments that make them quiet.
    ///
    /// PipeWire and PulseAudio first because they are what a desktop session
    /// actually runs; ALSA next for a bare machine; `afplay` for macOS; the
    /// media players last, since they are the least likely to be installed for
    /// this purpose and the most likely to open a window if asked wrong.
    const PLAYERS: &[(&str, &[&str])] = &[
        ("pw-play", &[]),
        ("paplay", &[]),
        ("aplay", &["-q"]),
        ("afplay", &[]),
        ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
        ("play", &["-q"]),
    ];

    /// A player that is currently sounding, and the file it is reading.
    #[derive(Debug)]
    pub struct Playing {
        child: Child,
        path: PathBuf,
        player: String,
    }

    impl Playing {
        /// Which program is making the sound, for the status line.
        #[must_use]
        pub fn player(&self) -> &str {
            &self.player
        }

        /// Whether it is still going. Reaping here is what keeps a finished
        /// player from lingering as a zombie until the app exits.
        pub fn finished(&mut self) -> bool {
            matches!(self.child.try_wait(), Ok(Some(_)) | Err(_))
        }

        /// Stop it now.
        pub fn stop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    impl Drop for Playing {
        fn drop(&mut self) {
            self.stop();
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Which player would be used, if any. Front ends show this so a machine
    /// with no player says so before the user presses play.
    #[must_use]
    pub fn available() -> Option<&'static str> {
        PLAYERS
            .iter()
            .find(|(program, _)| on_path(program))
            .map(|(program, _)| *program)
    }

    fn on_path(program: &str) -> bool {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
    }

    /// Write a WAV file somewhere it can be played from and read back.
    ///
    /// # Errors
    ///
    /// Returns the I/O error's message when the file cannot be written.
    pub fn write(path: &Path, wav: &[u8]) -> Result<(), String> {
        let mut file = std::fs::File::create(path)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        file.write_all(wav)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    }

    /// Play a WAV file through the first player on the machine.
    ///
    /// The file is written to the temporary directory under a name derived
    /// from the process and the sequence, so two Kaos windows never fight over
    /// one path, and it is removed when the handle is dropped.
    ///
    /// # Errors
    ///
    /// Returns a message naming the players it looked for when none is
    /// installed, or the spawn error when one is but will not start.
    pub fn play(wav: &[u8], sequence: u64) -> Result<Playing, String> {
        let Some((program, args)) = PLAYERS.iter().find(|(program, _)| on_path(program)) else {
            let names = PLAYERS
                .iter()
                .map(|(program, _)| *program)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "no audio player found — install one of: {names} (the wave can still be exported)"
            ));
        };
        let path =
            std::env::temp_dir().join(format!("kaos-music-{}-{sequence}.wav", std::process::id()));
        write(&path, wav)?;
        let child = Command::new(program)
            .args(*args)
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("{program} would not start: {error}"))?;
        Ok(Playing {
            child,
            path,
            player: (*program).to_string(),
        })
    }
}

/// One program, held ready to be heard.
///
/// Both front ends put a different face on exactly this: the window draws the
/// wave with egui and the terminal draws it with braille, but which program is
/// loaded, what it sounds like, whether it is playing and how far through — all
/// of that is decided here, once. A section that behaved differently in the two
/// front ends would be two features with one name.
#[derive(Default)]
pub struct Desk {
    /// What the listener chose.
    pub tuning: Tuning,
    /// The source last read, kept so a tuning change can re-derive without the
    /// caller having to hold the program too.
    pub source: String,
    /// The derived score, empty until a program is read.
    pub score: Score,
    /// The rendered piece. Re-rendered whenever the score or tuning changes,
    /// because it is what both the drawing and the player read.
    pub samples: Vec<f32>,
    /// The last thing that happened, for the status line.
    pub message: String,
    /// The wave as last reduced for drawing, and the width it was reduced to.
    ///
    /// Cached because a front end redraws far more often than a piece changes —
    /// the terminal repaints twenty times a second — and reducing two million
    /// samples on every frame is real CPU spent to arrive at the same picture.
    drawn: Option<(usize, Vec<Band>)>,
    playing: Option<sink::Playing>,
    started: Option<std::time::Instant>,
    sequence: u64,
}

impl std::fmt::Debug for Desk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Desk")
            .field("tuning", &self.tuning)
            .field("notes", &self.score.notes.len())
            .field("seconds", &self.score.seconds)
            .field("playing", &self.playing.is_some())
            .finish()
    }
}

impl Desk {
    /// Read a program, derive its score, and render it.
    ///
    /// # Errors
    ///
    /// Returns the parser's message when the source is not a program. The
    /// previous piece is left loaded in that case: a typo mid-edit should not
    /// silently empty the section that was playing.
    pub fn read(&mut self, source: &str) -> Result<(), String> {
        let score = Score::from_source(source, &self.tuning)?;
        self.source = source.to_string();
        self.score = score;
        self.samples = render(&self.score, &self.tuning);
        self.drawn = None;
        self.message = format!(
            "{} notes · {:.1}s · {} deep · {} at once",
            self.score.notes.len(),
            self.score.seconds,
            self.score.depth,
            self.score.polyphony()
        );
        Ok(())
    }

    /// Re-derive after the tuning changed. Silent when nothing is loaded.
    pub fn retune(&mut self) {
        if self.source.is_empty() {
            return;
        }
        let source = std::mem::take(&mut self.source);
        let _ = self.read(&source);
    }

    /// Whether a program is loaded and long enough to hear.
    #[must_use]
    pub fn loaded(&self) -> bool {
        !self.score.notes.is_empty()
    }

    /// Start playing from the beginning, replacing anything already sounding.
    pub fn play(&mut self) {
        if !self.loaded() {
            self.message = "nothing to play — read a program first".to_string();
            return;
        }
        self.stop();
        self.sequence += 1;
        match sink::play(&wav(&self.samples), self.sequence) {
            Ok(playing) => {
                self.message = format!("playing through {}", playing.player());
                self.playing = Some(playing);
                self.started = Some(std::time::Instant::now());
            }
            Err(error) => {
                self.message = error;
            }
        }
    }

    /// Stop, if anything is sounding.
    pub fn stop(&mut self) {
        if let Some(mut playing) = self.playing.take() {
            playing.stop();
            self.message = "stopped".to_string();
        }
        self.started = None;
    }

    /// Whether a player is still sounding. Reaps a finished one, so the
    /// playhead disappears when the piece ends rather than when it is noticed.
    pub fn sounding(&mut self) -> bool {
        let done = self
            .playing
            .as_mut()
            .is_none_or(|playing| playing.finished());
        if done {
            self.playing = None;
            self.started = None;
        }
        self.playing.is_some()
    }

    /// How far through the piece the player is, in seconds, or `None` when
    /// nothing is sounding.
    ///
    /// Read from the clock rather than from the audio device: nothing here
    /// owns the device, and a player started a moment ago is a moment into the
    /// file. Good to a frame, which is all a drawn playhead needs.
    pub fn playhead(&mut self) -> Option<f64> {
        if !self.sounding() {
            return None;
        }
        self.started.map(|at| at.elapsed().as_secs_f64())
    }

    /// The wave, reduced to as many columns as there is room to draw.
    ///
    /// Takes `&mut self` to keep the reduction — see [`Self::drawn`]. A panel
    /// that is redrawn without having changed pays for it once.
    pub fn bands(&mut self, columns: usize) -> Vec<Band> {
        if self
            .drawn
            .as_ref()
            .is_none_or(|(width, _)| *width != columns)
        {
            self.drawn = Some((columns, waveform(&self.samples, columns)));
        }
        self.drawn
            .as_ref()
            .map(|(_, bands)| bands.clone())
            .unwrap_or_default()
    }

    /// Write the piece as a WAV file.
    ///
    /// # Errors
    ///
    /// Returns a message when there is nothing loaded, or the I/O error.
    pub fn export(&mut self, path: &std::path::Path) -> Result<(), String> {
        if !self.loaded() {
            return Err("nothing to export — read a program first".to_string());
        }
        sink::write(path, &wav(&self.samples))?;
        self.message = format!("wrote {}", path.display());
        Ok(())
    }

    /// The Fibonacci reading of one token, for the panel that shows the
    /// working: the number it became, the Zeckendorf terms it decomposed to,
    /// and the degree those chose.
    #[must_use]
    pub fn reading(&self, token: &str) -> Reading {
        let table = fibonacci();
        let value = word_number(token);
        let indices = zeckendorf(value);
        Reading {
            value,
            degree: indices.iter().sum::<usize>() % SCALE_DEGREES,
            terms: indices.iter().map(|index| table[*index]).collect(),
            indices,
        }
    }
}

/// The arithmetic behind one tone, shown rather than asserted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// The word read as a Fibonacci-place number.
    pub value: u64,
    /// The Fibonacci numbers it decomposes into, largest first.
    pub terms: Vec<u64>,
    /// Their indices in the sequence — what actually chooses pitch and colour.
    pub indices: Vec<usize>,
    /// The scale degree those indices land on.
    pub degree: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zeckendorf_is_a_unique_sum_of_non_consecutive_fibonacci_numbers() {
        let table = fibonacci();
        for value in [
            1u64,
            2,
            3,
            4,
            12,
            100,
            1_000,
            4_181,
            999_999,
            u32::MAX as u64,
        ] {
            let indices = zeckendorf(value);
            let sum: u64 = indices.iter().map(|index| table[*index]).sum();
            assert_eq!(sum, value, "the decomposition of {value} does not sum back");
            for pair in indices.windows(2) {
                assert!(
                    pair[0] - pair[1] >= 2,
                    "consecutive terms in {value}: {indices:?}"
                );
            }
        }
        assert!(zeckendorf(0).is_empty(), "nothing decomposes to nothing");
    }

    #[test]
    fn a_word_is_a_number_that_hears_its_own_order() {
        // Positional, so an anagram is a different tone. A hash would do this
        // too; the point is that this one is a *place-value system* whose
        // places are the Fibonacci numbers, so the property is a consequence
        // of the reading rather than of stirring the bits.
        assert_ne!(word_number("abc"), word_number("cba"));
        assert_ne!(word_number("loop"), word_number("pool"));
        // Monotone in length: more characters is a larger number, and so —
        // through Zeckendorf — a richer tone.
        assert!(word_number("expansion") > word_number("x"));
        // Deterministic across runs and machines.
        assert_eq!(word_number("joy"), word_number("joy"));
    }

    #[test]
    fn the_scale_is_fibonacci_numbers_folded_into_one_octave() {
        let scale = scale();
        assert_eq!(scale.len(), SCALE_DEGREES);
        assert!(
            (scale[0] - 1.0).abs() < 1e-12,
            "the scale does not start on the root: {scale:?}"
        );
        assert!(
            scale.windows(2).all(|pair| pair[0] < pair[1]),
            "the degrees are not ordered: {scale:?}"
        );
        assert!(
            scale.iter().all(|degree| (1.0..2.0).contains(degree)),
            "a degree left the octave: {scale:?}"
        );
        // The famous ones are in there: 3/2 is the fifth, 5/4 the major third,
        // 13/8 the sixth. If these ever leave, the scale stopped being a
        // reading of the sequence and became a tuning someone chose.
        for ratio in [1.5, 1.25, 1.625, 1.125] {
            assert!(
                scale.iter().any(|degree| (degree - ratio).abs() < 1e-9),
                "{ratio} is not in the scale: {scale:?}"
            );
        }
    }

    #[test]
    fn time_is_indentation() {
        // One program, two symbols, written one level apart. The deeper one is
        // briefer — not because it holds less (it holds exactly as much) but
        // because it is written inside one more boundary, and a boundary is
        // time to be divided.
        let tuning = Tuning::default();
        // Written with prompts rather than symbols: `(a (b c))` is a *call* to
        // `a`, and the fixture is about indentation, not about calls.
        let score =
            Score::from_source("(\"a\" (\"b\" \"c\"))", &tuning).expect("the program parses");
        let note = |token: &str| {
            score
                .notes
                .iter()
                .find(|note| note.token == token)
                .unwrap_or_else(|| panic!("{token} did not sound: {:?}", score.notes))
                .clone()
        };
        let (shallow, deep) = (note("a"), note("c"));
        assert_eq!((shallow.depth, deep.depth), (1, 2));
        assert!(
            deep.duration < shallow.duration * 0.6,
            "the deeper form was not the briefer one: {} vs {}",
            deep.duration,
            shallow.duration
        );
        // Contained, too: a form cannot sound outside the indentation that
        // holds it, which is the other half of the same claim.
        let holder = note("(");
        let inner: Vec<&Note> = score.notes.iter().filter(|note| note.depth == 2).collect();
        let opens = inner.iter().map(|note| note.start).fold(f64::MAX, f64::min);
        assert!(
            opens >= holder.start,
            "a held form began before the boundary that holds it"
        );
        // And it sings higher: the octave is the indentation.
        assert!(deep.hz > shallow.hz, "depth did not raise the octave");
    }

    #[test]
    fn a_parallel_form_is_the_only_chord() {
        let tuning = Tuning::default();
        let series = Score::from_source("(-> a b)", &tuning).expect("parses");
        let parallel = Score::from_source("([m] a b)", &tuning).expect("parses");
        assert_eq!(
            series.polyphony(),
            1,
            "a chain sounded as a chord: {:?}",
            series.notes
        );
        assert!(
            parallel.polyphony() > 1,
            "the branches did not sound together: {:?}",
            parallel.notes
        );
    }

    #[test]
    fn a_prompt_sounds_as_its_words() {
        let tuning = Tuning::default();
        let score = Score::from_source("(\"joy and expansion\")", &tuning).expect("parses");
        let sung: Vec<&str> = score.notes.iter().map(|note| note.token.as_str()).collect();
        assert!(
            sung.contains(&"joy") && sung.contains(&"and") && sung.contains(&"expansion"),
            "the prompt did not sound word by word: {sung:?}"
        );
        // And they are in the order they were written.
        let joy = sung.iter().position(|token| *token == "joy");
        let expansion = sung.iter().position(|token| *token == "expansion");
        assert!(joy < expansion, "the words came out of order: {sung:?}");
    }

    #[test]
    fn minimal_is_a_sine_and_a_word_is_a_stack() {
        // The whole "atomic level" claim, as an assertion: one character is
        // very nearly a pure tone, and a word is many partials.
        let tuning = Tuning {
            partials: 6,
            ..Tuning::default()
        };
        let atom = tone("x", 0.0, 1.0, 0, &tuning);
        let word = tone("expansion", 0.0, 1.0, 0, &tuning);
        assert!(
            atom.partials.len() < word.partials.len(),
            "the atom was not simpler than the word: {:?} vs {:?}",
            atom.partials,
            word.partials
        );
        // Equal loudness, so growth is colour and not volume.
        let loudness = |note: &Note| note.partials.iter().map(|(_, gain)| gain).sum::<f64>();
        assert!((loudness(&atom) - loudness(&word)).abs() < 1e-9);
    }

    #[test]
    fn the_same_program_is_the_same_samples() {
        let tuning = Tuning::default();
        let source = "(~ greet (name) ($ \"hello \" name))";
        let once = render(&Score::from_source(source, &tuning).unwrap(), &tuning);
        let twice = render(&Score::from_source(source, &tuning).unwrap(), &tuning);
        assert_eq!(once, twice, "the synth is not deterministic");
        assert!(!once.is_empty(), "nothing was rendered");
        assert!(
            once.iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0),
            "a sample left the range"
        );
        assert!(
            once.iter().any(|sample| sample.abs() > 0.05),
            "the piece is silence"
        );
    }

    #[test]
    fn the_braille_wave_is_the_same_figure_the_window_draws() {
        // Many cycles per drawn column, swelling from nothing to full scale, so
        // each column's extremes are its envelope rather than wherever the
        // phase happened to be.
        let samples: Vec<f32> = (0..4000)
            .map(|index| (index as f32 * 0.8).sin() * (index as f32 / 4000.0))
            .collect();
        let rows = 4;
        let lines = braille(&waveform(&samples, 60 * 2), rows);
        assert_eq!(lines.len(), rows);
        assert!(
            lines.iter().all(|line| line.chars().count() == 60),
            "a row came out the wrong width: {:?}",
            lines
                .iter()
                .map(|line| line.chars().count())
                .collect::<Vec<_>>()
        );
        assert!(
            lines
                .iter()
                .all(|line| line.chars().all(|c| ('\u{2800}'..='\u{28ff}').contains(&c))),
            "something that is not braille was drawn"
        );
        // The wave swells, so the last column reaches rows the first does not.
        let ink = |line: &str, column: usize| line.chars().nth(column) != Some('\u{2800}');
        let filled = |column: usize| lines.iter().filter(|line| ink(line, column)).count();
        assert!(
            filled(59) > filled(0),
            "the drawn wave does not grow with the samples: {} vs {}",
            filled(59),
            filled(0)
        );
    }

    #[test]
    fn the_wav_header_is_a_wav_header() {
        let samples = vec![0.0f32, 0.5, -0.5, 1.0];
        let wav = wav(&samples);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + samples.len() * 2);
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
            wav.len() - 8,
            "the RIFF length does not describe the file"
        );
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            RATE,
            "the sample rate in the header is not the one rendered"
        );
    }

    #[test]
    fn the_waveform_covers_the_samples_it_is_drawn_from() {
        let samples: Vec<f32> = (0..1000).map(|index| (index as f32 / 50.0).sin()).collect();
        let bands = waveform(&samples, 40);
        assert_eq!(bands.len(), 40);
        let peak = bands.iter().fold(0f32, |peak, band| peak.max(band.high));
        assert!(peak > 0.99, "the drawn wave lost the peak: {peak}");
        assert!(
            bands.iter().all(|band| band.low <= band.high),
            "a band is inside out"
        );
        // A single column still describes the whole file rather than one sample.
        let whole = waveform(&samples, 1);
        assert!(whole[0].high > 0.99 && whole[0].low < -0.99);
    }

    #[test]
    fn an_empty_or_broken_program_is_refused_rather_than_played() {
        let tuning = Tuning::default();
        assert!(Score::from_source("(", &tuning).is_err());
        assert!(Score::from_source("", &tuning).is_err());
    }
}
