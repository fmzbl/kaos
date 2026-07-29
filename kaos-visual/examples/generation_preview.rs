//! Render a generation to a PNG, with no window and no compositor.
//!
//! The generation pane is a moving figure, and a moving figure cannot be
//! reviewed by reading its source. This renders the *same composition* the pane
//! paints — `Automaton::compose` is the single source of the geometry, and this
//! example only supplies a palette and a rasteriser — so the image is what the
//! pane shows, not an artist's impression of it.
//!
//! Use it to iterate on the composition on a headless machine, or to see what a
//! particular program and answer produce without launching the editor:
//!
//! ```text
//! cargo run --example generation_preview -- out.png '(["j"] "a" "b" "c")' 40
//! ```
//!
//! Arguments, all optional: output path, Rebis program, generations to advance,
//! and the word `refuse` to make a gate deny — the case that cuts a dark wedge
//! from the rim inward and is otherwise hard to reach on demand.
//! An answer is synthesised so the rule is not the default one — a real run's
//! answers are what build the rule, and a preview with no answer would show the
//! lattice evolving on the fallback table.

use kaos_visual::automata_preview::{Automaton, Mark, Ramp};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "generation.png".to_string());
    let program = args.next().unwrap_or_else(|| {
        // A program with one of everything, so the preview exercises every form:
        // a square with a symbol mediator, an arrow chain, a lazy gate.
        r#"([judge]
             (-> "read the tape" "name the standard")
             (% (gate "is there a counterparty") "size it" "refuse")
             ([reconcile] "fast frame" "slow frame")
             "the imputation")"#
            .to_string()
    });
    let generations: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(40);
    let refuse = args.next().is_some_and(|word| word == "refuse");

    let expr = match rebis_lang::parse(&program) {
        Ok(expr) => expr,
        Err(error) => {
            eprintln!("that program does not parse, so it has no lattice: {error}");
            std::process::exit(2);
        }
    };

    let mut machine = Automaton::from_program(&expr);
    // Stand in for a run: seed each prompt, then let one answer build the rule.
    for prompt in [
        "read the tape",
        "name the standard",
        "size it",
        "fast frame",
    ] {
        machine.observe_prompt(prompt);
    }
    machine.observe_answer(
        "The fifteen-minute frame is inside a four-hour expansion, so the reversal \
         is noise within a larger structure. Guaranteed minimum net of cost is \
         negative; the interval contains zero. Refuse.",
    );
    // Halfway through, so the wedge starts mid-figure and the generations before
    // the refusal are still visible inside it.
    for step in 0..generations {
        if refuse && step == generations / 2 {
            machine.observe_refusal();
        }
        machine.step();
    }

    const SIZE: usize = 900;
    // The pane's own dark palette, so the preview reads like the pane does.
    let palette = kaos_core::theme::palette(kaos_core::theme::Mode::Dark);
    let ground = palette.ground;
    let faint = palette.faint;
    let ink = palette.ink;
    let accent = palette.accent;
    let secondary = palette.secondary;

    let mut canvas = Canvas::new(SIZE, SIZE, ground);
    let centre = (SIZE as f32 / 2.0, SIZE as f32 / 2.0);
    let extent = SIZE as f32 / 2.0 - 34.0;

    // The ramp itself comes from the module, so this cannot drift from the pane.
    let tint = |state: u8| -> (u8, u8, u8) {
        let (from, to, local) = match kaos_visual::automata_preview::ramp(state) {
            Ramp::Dim(t) => (ground, faint, t),
            Ramp::Quiet(t) => (faint, ink, t),
            Ramp::Loud(t) => (ink, accent, t),
        };
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * local) as u8;
        (lerp(from.0, to.0), lerp(from.1, to.1), lerp(from.2, to.2))
    };

    for mark in machine.compose(centre, extent) {
        match mark {
            Mark::Dot {
                at,
                radius,
                state,
                alpha,
                filled,
            } => {
                if filled {
                    canvas.disc(at, radius, tint(state), alpha);
                } else {
                    canvas.ring(at, radius, tint(state), alpha);
                }
            }
            Mark::Line {
                from,
                to,
                width,
                state,
                alpha,
            } => canvas.line(from, to, width, tint(state), alpha),
            Mark::Poly {
                points,
                state,
                alpha,
                filled,
                width,
            } => {
                let colour = tint(state);
                if filled {
                    canvas.polygon(&points, colour, alpha);
                } else {
                    for pair in 0..points.len() {
                        let next = (pair + 1) % points.len();
                        canvas.line(points[pair], points[next], width, colour, alpha);
                    }
                }
            }
            Mark::Cross {
                at,
                arm,
                state,
                alpha,
            } => {
                let colour = tint(state);
                canvas.line(
                    (at.0 - arm, at.1 - arm),
                    (at.0 + arm, at.1 + arm),
                    1.0,
                    colour,
                    alpha,
                );
                canvas.line(
                    (at.0 - arm, at.1 + arm),
                    (at.0 + arm, at.1 - arm),
                    1.0,
                    colour,
                    alpha,
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
                let base = match stream {
                    kaos_visual::automata_preview::BinaryStream::Prompt => secondary,
                    kaos_visual::automata_preview::BinaryStream::Response => accent,
                };
                let weight = 0.35 + value as f32 / 255.0 * 0.65;
                canvas.line(
                    from,
                    to,
                    if on { 2.0 } else { 0.65 },
                    (
                        (base.0 as f32 * weight) as u8,
                        (base.1 as f32 * weight) as u8,
                        (base.2 as f32 * weight) as u8,
                    ),
                    alpha,
                );
            }
        }
    }

    match std::fs::write(&path, canvas.png()) {
        Ok(()) => println!(
            "{path} · {} cells · generation {} · H {:.2} bits/byte · {} killed",
            machine.cells.len(),
            machine.generation,
            machine.entropy,
            machine.dead_count()
        ),
        Err(error) => {
            eprintln!("could not write {path}: {error}");
            std::process::exit(1);
        }
    }
}

// ── a very small rasteriser ─────────────────────────────────────────────────
//
// Deliberately dependency-free: this is a development aid, and adding an image
// crate to the editor's dependency tree to look at a picture would be a poor
// trade. Anti-aliasing is by 3×3 supersampling of coverage, which is enough to
// judge a composition.

struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: usize, height: usize, ground: (u8, u8, u8)) -> Self {
        let mut pixels = Vec::with_capacity(width * height * 3);
        for _ in 0..width * height {
            pixels.extend_from_slice(&[ground.0, ground.1, ground.2]);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    fn blend(&mut self, x: isize, y: isize, colour: (u8, u8, u8), alpha: f32) {
        if x < 0 || y < 0 || x >= self.width as isize || y >= self.height as isize {
            return;
        }
        let alpha = alpha.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }
        let at = (y as usize * self.width + x as usize) * 3;
        for (channel, value) in [colour.0, colour.1, colour.2].into_iter().enumerate() {
            let under = self.pixels[at + channel] as f32;
            self.pixels[at + channel] = (under + (value as f32 - under) * alpha) as u8;
        }
    }

    /// Coverage of one pixel by a shape, sampled 3×3.
    fn coverage(x: isize, y: isize, inside: &dyn Fn(f32, f32) -> bool) -> f32 {
        let mut hits = 0;
        for sy in 0..3 {
            for sx in 0..3 {
                let px = x as f32 + (sx as f32 + 0.5) / 3.0;
                let py = y as f32 + (sy as f32 + 0.5) / 3.0;
                if inside(px, py) {
                    hits += 1;
                }
            }
        }
        hits as f32 / 9.0
    }

    fn each_pixel_in(
        &mut self,
        min: (f32, f32),
        max: (f32, f32),
        colour: (u8, u8, u8),
        alpha: f32,
        inside: &dyn Fn(f32, f32) -> bool,
    ) {
        let x0 = min.0.floor() as isize - 1;
        let y0 = min.1.floor() as isize - 1;
        let x1 = max.0.ceil() as isize + 1;
        let y1 = max.1.ceil() as isize + 1;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let hit = Self::coverage(x, y, inside);
                if hit > 0.0 {
                    self.blend(x, y, colour, alpha * hit);
                }
            }
        }
    }

    fn disc(&mut self, at: (f32, f32), radius: f32, colour: (u8, u8, u8), alpha: f32) {
        let r2 = radius * radius;
        self.each_pixel_in(
            (at.0 - radius, at.1 - radius),
            (at.0 + radius, at.1 + radius),
            colour,
            alpha,
            &|x, y| (x - at.0).powi(2) + (y - at.1).powi(2) <= r2,
        );
    }

    fn ring(&mut self, at: (f32, f32), radius: f32, colour: (u8, u8, u8), alpha: f32) {
        let outer = (radius + 0.6).powi(2);
        let inner = (radius - 0.6).max(0.0).powi(2);
        self.each_pixel_in(
            (at.0 - radius - 1.0, at.1 - radius - 1.0),
            (at.0 + radius + 1.0, at.1 + radius + 1.0),
            colour,
            alpha,
            &|x, y| {
                let d = (x - at.0).powi(2) + (y - at.1).powi(2);
                d <= outer && d >= inner
            },
        );
    }

    fn line(
        &mut self,
        from: (f32, f32),
        to: (f32, f32),
        width: f32,
        colour: (u8, u8, u8),
        alpha: f32,
    ) {
        let half = (width / 2.0).max(0.5);
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = (dx * dx + dy * dy).sqrt();
        if length < f32::EPSILON {
            return self.disc(from, half, colour, alpha);
        }
        self.each_pixel_in(
            (from.0.min(to.0) - half, from.1.min(to.1) - half),
            (from.0.max(to.0) + half, from.1.max(to.1) + half),
            colour,
            alpha,
            &|x, y| {
                // Distance to the segment, clamped to its ends.
                let t =
                    (((x - from.0) * dx + (y - from.1) * dy) / (length * length)).clamp(0.0, 1.0);
                let (nx, ny) = (from.0 + dx * t, from.1 + dy * t);
                ((x - nx).powi(2) + (y - ny).powi(2)).sqrt() <= half
            },
        );
    }

    fn polygon(&mut self, points: &[(f32, f32)], colour: (u8, u8, u8), alpha: f32) {
        if points.len() < 3 {
            return;
        }
        let min = points.iter().fold((f32::MAX, f32::MAX), |acc, p| {
            (acc.0.min(p.0), acc.1.min(p.1))
        });
        let max = points.iter().fold((f32::MIN, f32::MIN), |acc, p| {
            (acc.0.max(p.0), acc.1.max(p.1))
        });
        self.each_pixel_in(min, max, colour, alpha, &|x, y| {
            // Even-odd crossing test.
            let mut inside = false;
            let mut j = points.len() - 1;
            for i in 0..points.len() {
                let (xi, yi) = points[i];
                let (xj, yj) = points[j];
                if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                    inside = !inside;
                }
                j = i;
            }
            inside
        });
    }

    /// Encode as a PNG with stored (uncompressed) deflate blocks — no zlib
    /// dependency, and the file is only read by a person looking at it.
    fn png(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(self.height * (1 + self.width * 3));
        for row in 0..self.height {
            raw.push(0); // filter: none
            let at = row * self.width * 3;
            raw.extend_from_slice(&self.pixels[at..at + self.width * 3]);
        }

        let mut zlib = vec![0x78, 0x01];
        for (index, chunk) in raw.chunks(65_535).enumerate() {
            let last = (index + 1) * 65_535 >= raw.len();
            zlib.push(u8::from(last));
            zlib.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
            zlib.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
            zlib.extend_from_slice(chunk);
        }
        zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut header = Vec::new();
        header.extend_from_slice(&(self.width as u32).to_be_bytes());
        header.extend_from_slice(&(self.height as u32).to_be_bytes());
        header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour RGB
        chunk(&mut out, b"IHDR", &header);
        chunk(&mut out, b"IDAT", &zlib);
        chunk(&mut out, b"IEND", &[]);
        out
    }
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let mut crc_over = kind.to_vec();
    crc_over.extend_from_slice(body);
    out.extend_from_slice(&crc32(&crc_over).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
