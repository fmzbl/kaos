//! Drawn sigils: strokes, and the image a model is shown.
//!
//! The mandala renders a program — every node is a form and the picture is the
//! code. This is the other direction. A sigil here is a DRAWING, and nothing in
//! it corresponds to anything in the language: it is compressed intent, the way
//! a chaos sigil is, and the model is asked to read it rather than to decode it.
//!
//! The two must not be confused, so they share no type. A mandala can be turned
//! back into source and checked against what it came from; a sigil cannot, and
//! asking it to would be asking a drawing to be a program.
//!
//! # Why this is a value rather than a picture on a screen
//!
//! What a program actually receives is `(&: "…png")` — bytes on disk, carried
//! to the model by the same path a screenshot takes. So strokes have to become
//! an image file, and the file is the artefact. Keeping the raster here rather
//! than in the canvas means the whole path is testable without a window.
//!
//! # The PNG is written by hand
//!
//! Deflate permits *stored* blocks — uncompressed, with a length and its
//! complement — and a PNG whose IDAT uses them is an ordinary PNG that every
//! decoder reads. That is about a hundred lines and no dependency, against a
//! general image crate for one format written once. The workspace has no image
//! dependency and this does not add one.

use std::path::Path;

/// One continuous mark: where the pointer went while it was down.
///
/// Points are in the drawing's own coordinates, origin top-left, and are not
/// scaled until the raster is made — so a sigil drawn on a large canvas and one
/// drawn on a small one produce the same image.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stroke {
    pub points: Vec<(f32, f32)>,
    /// How wide the mark is, in pixels of the finished image.
    pub width: f32,
}

impl Stroke {
    #[must_use]
    pub fn new(width: f32) -> Stroke {
        Stroke {
            points: Vec::new(),
            width: width.max(1.0),
        }
    }

    pub fn add(&mut self, x: f32, y: f32) {
        // A pointer that has not moved contributes nothing but cost.
        if let Some((last_x, last_y)) = self.points.last() {
            if (last_x - x).abs() < 0.5 && (last_y - y).abs() < 0.5 {
                return;
            }
        }
        self.points.push((x, y));
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// A whole drawing: the strokes of one sigil.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sigil {
    pub strokes: Vec<Stroke>,
}

impl Sigil {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.strokes.iter().all(Stroke::is_empty)
    }

    /// The bounding box of every mark, or `None` for an empty sigil.
    #[must_use]
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut seen = false;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for point in self.strokes.iter().flat_map(|s| s.points.iter()) {
            seen = true;
            min_x = min_x.min(point.0);
            min_y = min_y.min(point.1);
            max_x = max_x.max(point.0);
            max_y = max_y.max(point.1);
        }
        seen.then_some((min_x, min_y, max_x, max_y))
    }

    /// Rasterise to a square image, black on white, with the drawing centred
    /// and scaled to fill it.
    ///
    /// Black on white rather than the editor's palette: what a model reads best
    /// is maximum contrast, and a sigil's meaning is in its shape rather than
    /// its colour. Cropping to the bounds and scaling means a mark made in the
    /// corner of a big canvas arrives the same size as one made in the middle
    /// of a small one — the model sees the sigil, not the canvas it was drawn
    /// on.
    #[must_use]
    pub fn raster(&self, size: usize) -> Raster {
        let size = size.clamp(32, 2048);
        let mut raster = Raster::blank(size);
        let Some((min_x, min_y, max_x, max_y)) = self.bounds() else {
            return raster;
        };
        // A margin, so strokes never touch the edge and get read as a border.
        let margin = size as f32 * 0.08;
        let span = (max_x - min_x).max(max_y - min_y).max(1.0);
        let scale = (size as f32 - margin * 2.0) / span;
        let (centre_x, centre_y) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
        let place = |x: f32, y: f32| {
            (
                (x - centre_x) * scale + size as f32 / 2.0,
                (y - centre_y) * scale + size as f32 / 2.0,
            )
        };
        for stroke in &self.strokes {
            let width = (stroke.width * scale).clamp(1.5, size as f32 / 16.0);
            // A single tap is a dot, not nothing.
            if stroke.points.len() == 1 {
                let (x, y) = place(stroke.points[0].0, stroke.points[0].1);
                raster.dot(x, y, width);
                continue;
            }
            for pair in stroke.points.windows(2) {
                let (from, to) = (place(pair[0].0, pair[0].1), place(pair[1].0, pair[1].1));
                raster.line(from, to, width);
            }
        }
        raster
    }
}

/// An 8-bit greyscale image.
///
/// Greyscale rather than colour: one channel is a third of the bytes and a
/// sigil has no colour to lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    pub size: usize,
    /// Row-major, `0` black and `255` white.
    pub pixels: Vec<u8>,
}

impl Raster {
    #[must_use]
    pub fn blank(size: usize) -> Raster {
        Raster {
            size,
            pixels: vec![255; size * size],
        }
    }

    fn ink(&mut self, x: isize, y: isize, coverage: f32) {
        if x < 0 || y < 0 || x as usize >= self.size || y as usize >= self.size {
            return;
        }
        let index = y as usize * self.size + x as usize;
        let darkened = f32::from(self.pixels[index]) * (1.0 - coverage.clamp(0.0, 1.0));
        // Darkest wins, so overlapping strokes never lighten each other.
        self.pixels[index] = self.pixels[index].min(darkened.round() as u8);
    }

    /// A filled disc, which is both a dot and the round cap of a line.
    fn dot(&mut self, cx: f32, cy: f32, width: f32) {
        let radius = width / 2.0;
        let reach = radius.ceil() as isize + 1;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                let (x, y) = (cx.round() as isize + dx, cy.round() as isize + dy);
                let distance = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                // One pixel of falloff at the rim, so a curve does not look
                // like a staircase.
                let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.ink(x, y, coverage);
                }
            }
        }
    }

    /// A stroke segment, drawn as overlapping discs.
    ///
    /// Slower than a scanline fill and far shorter; a sigil is a few hundred
    /// segments at most, and this is not on any path that runs per frame.
    fn line(&mut self, from: (f32, f32), to: (f32, f32), width: f32) {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = (dx * dx + dy * dy).sqrt();
        if !length.is_finite() {
            return;
        }
        let steps = (length * 2.0).ceil().max(1.0) as usize;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            self.dot(from.0 + dx * t, from.1 + dy * t, width);
        }
    }

    /// The image as PNG bytes.
    #[must_use]
    pub fn to_png(&self) -> Vec<u8> {
        png_greyscale(self.size, &self.pixels)
    }

    /// Write the image beside whatever else is on disk.
    ///
    /// # Errors
    ///
    /// When the directory cannot be created or the file cannot be written.
    pub fn write_png(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_png())
    }
}

// ── PNG, by hand ──────────────────────────────────────────────────────────

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut checked = Vec::with_capacity(4 + body.len());
    checked.extend_from_slice(kind);
    checked.extend_from_slice(body);
    out.extend_from_slice(&checked);
    out.extend_from_slice(&crc32(&checked).to_be_bytes());
}

/// An 8-bit greyscale PNG whose image data uses stored deflate blocks.
///
/// Stored blocks are uncompressed — a header, a length, its complement, then
/// the bytes — and are as legal as any other deflate stream. The file is larger
/// than a compressed one and every decoder reads it, which is the trade a
/// hundred lines buys against a dependency.
fn png_greyscale(size: usize, pixels: &[u8]) -> Vec<u8> {
    // Each row is prefixed with its filter byte; 0 is "no filter".
    let mut raw = Vec::with_capacity((size + 1) * size);
    for row in 0..size {
        raw.push(0);
        raw.extend_from_slice(&pixels[row * size..(row + 1) * size]);
    }

    // zlib: header, stored deflate blocks, adler of the uncompressed data.
    let mut zlib = vec![0x78, 0x01];
    let mut rest = raw.as_slice();
    while !rest.is_empty() {
        let take = rest.len().min(65_535);
        let (block, remaining) = rest.split_at(take);
        zlib.push(u8::from(remaining.is_empty()));
        zlib.extend_from_slice(&(take as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(take as u16)).to_le_bytes());
        zlib.extend_from_slice(block);
        rest = remaining;
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(size as u32).to_be_bytes());
    header.extend_from_slice(&(size as u32).to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit, greyscale, no interlace
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scribble() -> Sigil {
        let mut stroke = Stroke::new(3.0);
        for step in 0..40 {
            let t = step as f32 / 39.0;
            stroke.add(10.0 + t * 80.0, 20.0 + (t * 6.0).sin() * 30.0);
        }
        Sigil {
            strokes: vec![stroke],
        }
    }

    #[test]
    fn a_drawing_becomes_a_png_every_decoder_can_read() {
        let png = scribble().raster(128).to_png();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        // IHDR immediately after the signature, IEND at the end.
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        // Every chunk's CRC has to check out, or a decoder rejects the file.
        let mut at = 8;
        let mut kinds = Vec::new();
        while at + 8 <= png.len() {
            let length = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let checked = &png[at + 4..at + 8 + length];
            let stated =
                u32::from_be_bytes(png[at + 8 + length..at + 12 + length].try_into().unwrap());
            assert_eq!(crc32(checked), stated, "chunk CRC");
            kinds.push(String::from_utf8_lossy(&png[at + 4..at + 8]).into_owned());
            at += 12 + length;
        }
        assert_eq!(kinds, vec!["IHDR", "IDAT", "IEND"]);
        assert_eq!(at, png.len(), "no trailing bytes");
    }

    #[test]
    fn a_drawing_leaves_marks_and_an_empty_one_does_not() {
        let drawn = scribble().raster(96);
        assert!(
            drawn.pixels.iter().any(|p| *p < 40),
            "the stroke should be dark somewhere"
        );
        assert!(
            drawn.pixels.contains(&255),
            "and the page should still be mostly white"
        );
        let blank = Sigil::default().raster(96);
        assert!(blank.pixels.iter().all(|p| *p == 255));
        assert!(Sigil::default().is_empty());
    }

    /// A mark made in the corner of a large canvas must arrive the same size as
    /// one made in the middle of a small one. The model is shown the sigil, not
    /// the surface it happened to be drawn on.
    #[test]
    fn the_drawing_is_centred_and_scaled_rather_than_the_canvas() {
        // The same picture at two scales, a thousand pixels apart: the pen
        // widens with the drawing, or one would be relatively fatter than the
        // other and they would not be the same picture to compare.
        let mut near = Stroke::new(2.0);
        near.add(0.0, 0.0);
        near.add(10.0, 10.0);
        let mut far = Stroke::new(20.0);
        far.add(900.0, 900.0);
        far.add(1000.0, 1000.0);

        let one = Sigil {
            strokes: vec![near],
        }
        .raster(64);
        let other = Sigil { strokes: vec![far] }.raster(64);
        let darkness = |r: &Raster| r.pixels.iter().filter(|p| **p < 128).count();
        // Same shape, ten times the size and a thousand pixels away: the same
        // picture, give or take rounding at the rim.
        let (a, b) = (darkness(&one), darkness(&other));
        assert!(a > 0 && b > 0, "both should have ink: {a} {b}");
        assert!(
            (a as f32 - b as f32).abs() / (a as f32) < 0.25,
            "the two should be within a quarter of each other: {a} vs {b}"
        );
    }

    #[test]
    fn a_single_tap_is_a_dot_rather_than_nothing() {
        let mut tap = Stroke::new(4.0);
        tap.add(50.0, 50.0);
        let raster = Sigil { strokes: vec![tap] }.raster(64);
        assert!(
            raster.pixels.iter().any(|p| *p < 128),
            "a tap leaves a mark"
        );
    }

    #[test]
    fn a_pointer_that_has_not_moved_adds_no_points() {
        let mut stroke = Stroke::new(2.0);
        stroke.add(10.0, 10.0);
        stroke.add(10.1, 10.1);
        stroke.add(20.0, 20.0);
        assert_eq!(stroke.points.len(), 2);
    }
}

#[cfg(test)]
mod written {
    use super::*;

    /// Write one to disk so it can be opened by something that is not us.
    #[test]
    fn a_sigil_reaches_disk_as_a_file() {
        let dir = std::env::temp_dir().join(format!("kaos-ink-{}", std::process::id()));
        let path = dir.join("sigil.png");
        let mut stroke = Stroke::new(4.0);
        for step in 0..60 {
            let t = step as f32 / 59.0;
            let angle = t * std::f32::consts::TAU;
            stroke.add(50.0 + angle.cos() * 30.0, 50.0 + (angle * 3.0).sin() * 25.0);
        }
        let sigil = Sigil {
            strokes: vec![stroke],
        };
        sigil.raster(256).write_png(&path).expect("write");
        let bytes = std::fs::read(&path).expect("read back");
        assert!(bytes.len() > 1000, "a drawn sigil is not a trivial file");
        assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ── where drawings live ───────────────────────────────────────────────────

/// A flat wall of drawn sigils.
///
/// **Flat on purpose.** Programs are namespaced because they import each other
/// — `(# archetypes/shadow)` has to resolve, and folders are what make that
/// work. A drawing imports nothing. It is a mark with a name, and giving it a
/// folder tree would be inventing structure the thing does not have.
///
/// So this is one directory, one file per sigil, and a name is a name. The
/// module namespace in [`crate::sigils`] is untouched; the two stores answer
/// different questions and only one of them needs paths.
///
/// Each sigil is kept twice: the strokes, so it can be drawn on again, and the
/// PNG, because that is what a model is actually shown. Regenerating the image
/// from the strokes on every run would work and would mean the file a program
/// names could differ from the file that was reviewed.
pub struct Wall {
    root: std::path::PathBuf,
}

/// Why a drawn sigil could not be stored or fetched.
#[derive(Debug)]
pub enum WallError {
    /// The name would escape the wall, or is empty.
    Name(String),
    /// The filesystem refused.
    Io(std::io::Error),
    /// The stroke file is not one of ours.
    Unreadable(String),
}

impl std::fmt::Display for WallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name(name) => write!(f, "`{name}` is not a usable sigil name"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Unreadable(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for WallError {}

impl From<std::io::Error> for WallError {
    fn from(error: std::io::Error) -> WallError {
        WallError::Io(error)
    }
}

impl Wall {
    #[must_use]
    pub fn new(root: impl Into<std::path::PathBuf>) -> Wall {
        Wall { root: root.into() }
    }

    /// The user's own wall, beside the sigil library.
    #[must_use]
    pub fn default_wall() -> Wall {
        Wall::new(
            dirs_home()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".kaos/drawn"),
        )
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A name with nowhere to hide.
    ///
    /// Names are data. Flat means flat: no separators, no dots, nothing that
    /// could climb out of the directory. A drawing called `../../.ssh/id_rsa`
    /// must not be able to reach anything.
    fn stem(&self, name: &str) -> Result<String, WallError> {
        let trimmed = name.trim();
        let usable = !trimmed.is_empty()
            && trimmed.len() <= 64
            && trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if usable {
            Ok(trimmed.to_string())
        } else {
            Err(WallError::Name(name.to_string()))
        }
    }

    /// Where a sigil's drawing and picture sit.
    ///
    /// # Errors
    ///
    /// When the name is not usable.
    pub fn paths(&self, name: &str) -> Result<(std::path::PathBuf, std::path::PathBuf), WallError> {
        let stem = self.stem(name)?;
        Ok((
            self.root.join(format!("{stem}.sigil")),
            self.root.join(format!("{stem}.png")),
        ))
    }

    /// Keep a drawing, and the image a model will be shown.
    ///
    /// # Errors
    ///
    /// When the name is unusable or the filesystem refuses.
    pub fn save(
        &self,
        name: &str,
        sigil: &Sigil,
        size: usize,
    ) -> Result<std::path::PathBuf, WallError> {
        let (strokes, picture) = self.paths(name)?;
        std::fs::create_dir_all(&self.root)?;
        std::fs::write(&strokes, write_strokes(sigil))?;
        sigil.raster(size).write_png(&picture)?;
        Ok(picture)
    }

    /// Fetch a drawing so it can be drawn on again.
    ///
    /// # Errors
    ///
    /// When the name is unusable, the file is missing, or it is not ours.
    pub fn load(&self, name: &str) -> Result<Sigil, WallError> {
        let (strokes, _) = self.paths(name)?;
        read_strokes(&std::fs::read_to_string(strokes)?)
    }

    /// Every sigil on the wall, in name order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut found: Vec<String> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension()? == "sigil")
                    .then(|| path.file_stem()?.to_str().map(str::to_string))?
            })
            .collect();
        found.sort();
        found
    }

    /// Forget one.
    ///
    /// # Errors
    ///
    /// When the name is unusable.
    pub fn forget(&self, name: &str) -> Result<(), WallError> {
        let (strokes, picture) = self.paths(name)?;
        let _ = std::fs::remove_file(strokes);
        let _ = std::fs::remove_file(picture);
        Ok(())
    }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// Strokes as lines of text.
///
/// Hand-written rather than a serialisation dependency, for the same reason as
/// the PNG: one format, written once, and the workspace stays free of it. It is
/// also readable, which matters for a file a person might want to inspect after
/// a drawing comes back wrong.
fn write_strokes(sigil: &Sigil) -> String {
    let mut out = String::from("kaos-sigil 1\n");
    for stroke in &sigil.strokes {
        if stroke.is_empty() {
            continue;
        }
        out.push_str(&format!("stroke {:.3}\n", stroke.width));
        for (x, y) in &stroke.points {
            out.push_str(&format!("{x:.2} {y:.2}\n"));
        }
    }
    out
}

fn read_strokes(text: &str) -> Result<Sigil, WallError> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("kaos-sigil 1") {
        return Err(WallError::Unreadable(
            "not a kaos sigil — the first line should read `kaos-sigil 1`".to_string(),
        ));
    }
    let mut sigil = Sigil::default();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(width) = line.strip_prefix("stroke ") {
            let width = width
                .parse()
                .map_err(|_| WallError::Unreadable(format!("`{line}` is not a stroke width")))?;
            sigil.strokes.push(Stroke::new(width));
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(x), Some(y), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(WallError::Unreadable(format!("`{line}` is not a point")));
        };
        let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) else {
            return Err(WallError::Unreadable(format!("`{line}` is not a point")));
        };
        let Some(stroke) = sigil.strokes.last_mut() else {
            return Err(WallError::Unreadable(
                "a point appeared before any stroke".to_string(),
            ));
        };
        stroke.points.push((x, y));
    }
    Ok(sigil)
}

// ── from a stack of drawings to a program ─────────────────────────────────

/// The pending sigils, in the order they were drawn.
///
/// Drawings stack before anything runs. That is the whole interaction: you make
/// several marks, and only then ask — so intent is composed before it is spent,
/// which is the same discipline the language applies to everything else.
#[derive(Debug, Default, Clone)]
pub struct Stack {
    /// Paths to the pictures, in drawing order.
    pub pictures: Vec<std::path::PathBuf>,
    /// What the sigils are about, in the drawer's own words. Optional — a
    /// sigil that needed a sentence to explain it is a sentence.
    pub said: String,
}

impl Stack {
    pub fn push(&mut self, picture: std::path::PathBuf) {
        self.pictures.push(picture);
    }

    pub fn clear(&mut self) {
        self.pictures.clear();
        self.said.clear();
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pictures.is_empty()
    }

    /// The Rebis program that asks a model to read the stack and write one.
    ///
    /// Nothing here is new. `&:` loads a file and carries its bytes; `+` puts
    /// that value in scope so it reaches a prompt; `><` is a prompt whose
    /// answer is a program, and it parses the answer or reports a diagnostic.
    /// Stacked drawings are nested framings, because that is what "all of these
    /// are in scope at once" already means.
    ///
    /// ```text
    /// (+ (&: "one.png")
    ///    (+ (&: "two.png")
    ///       (>< "…")))
    /// ```
    ///
    /// # What this does NOT promise
    ///
    /// `><` guarantees the answer parses. It guarantees nothing about whether
    /// the program means what was drawn — a sigil is compressed intent and the
    /// decompression is a guess. So the generated source is something to read
    /// before running, and the interaction is built around that rather than
    /// around trusting it.
    #[must_use]
    pub fn program(&self) -> String {
        let mut inner = String::from("(>< ");
        inner.push_str(&quoted(&self.instruction()));
        inner.push(')');
        for picture in self.pictures.iter().rev() {
            inner = format!("(+ (&: {}) {inner})", quoted(&picture.to_string_lossy()));
        }
        inner
    }

    /// What the model is told about the marks.
    ///
    /// Deliberately short, and deliberately does not explain what a sigil is
    /// for. A sigil is compressed intent; telling the model what to decompress
    /// it INTO is the whole instruction, and everything else is decoration that
    /// would compete with the drawing for attention.
    #[must_use]
    pub fn instruction(&self) -> String {
        let count = self.pictures.len();
        let marks = if count == 1 {
            "The attached image is a sigil: a drawn mark standing for an intent.".to_string()
        } else {
            format!(
                "The {count} attached images are sigils: drawn marks standing for one intent \
                 between them, in the order given."
            )
        };
        let mut said = String::new();
        if !self.said.trim().is_empty() {
            said = format!("\n\nThe person drawing them said: {}", self.said.trim());
        }
        format!(
            "{marks} Read them and write the Rebis program they ask for.\n\n\
             Reply with ONLY the program as Rebis source — no prose, no fences, no \
             explanation. If the marks do not determine something you need, write the \
             program you can and leave a `;` comment naming the one thing you would want \
             another sigil for.{said}"
        )
    }
}

/// A Rebis prompt literal.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
