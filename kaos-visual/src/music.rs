//! The Sound tab: a program heard, and the wave it makes.
//!
//! Nothing musical is decided here. The mapping from a program to notes, the
//! synth, the wave and the player all live in `kaos_core::music`, which the
//! terminal front end wears as well — this module is the face: a wave to look
//! at, a playhead, four knobs, and the arithmetic of whichever note is under
//! the pointer, written out so the mapping can be *read* rather than believed.
//!
//! The tab holds its own copy of the source it was opened from. A drawing goes
//! on being edited while its music is open, and a piece that re-derived itself
//! under a half-finished edit would be a different piece every keystroke; `re-read`
//! is the gesture that takes the new version, deliberately.

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};

use kaos_core::music::{Desk, Note, Timbre};

use crate::theme::Ink;

/// One open Sound tab.
pub(crate) struct MusicPane {
    /// The shared desk: score, samples, player, tuning.
    pub(crate) desk: Desk,
    /// Where the source came from, for the header and the tab title.
    pub(crate) origin: String,
    /// The tab the program was taken from, so `re-read` knows where to look.
    /// `None` once that tab is closed, or when the program came from a run.
    pub(crate) from: Option<kaos_core::tabs::TabId>,
    /// The note the pointer is over, as an index into the score. Its Fibonacci
    /// reading is written beneath the wave.
    pub(crate) reading: Option<usize>,
    /// Whether the piece restarts as soon as the player finishes.
    pub(crate) looping: bool,
}

impl MusicPane {
    /// Open a pane on a program, or say why it cannot be heard.
    pub(crate) fn from_source(
        source: &str,
        origin: String,
        from: Option<kaos_core::tabs::TabId>,
    ) -> Result<Self, String> {
        let mut desk = Desk::default();
        desk.read(source)?;
        Ok(Self {
            desk,
            origin,
            from,
            reading: None,
            looping: false,
        })
    }
}

/// How tall the wave is drawn, in points. Enough to see the envelope of a
/// single note at one end of the piece and the shape of a phrase at the other.
const WAVE_HEIGHT: f32 = 190.0;

/// Draw the whole tab. Returns a new source to adopt when `re-read` was
/// pressed, since the pane cannot reach the editor's other tabs itself.
/// `full` is false in a torn-off window, where the two controls that need the
/// editor — re-reading the program from another tab, and the save dialog —
/// cannot be honoured. They are left out rather than drawn dead: a button that
/// does nothing is worse than a button that is not there.
pub(crate) fn tab(ui: &mut egui::Ui, pane: &mut MusicPane, k: &Ink, full: bool) -> Reaction {
    let mut reaction = Reaction::default();
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(k.mid, "SOUND ·");
        ui.colored_label(k.ink, &pane.origin);
        ui.separator();
        let sounding = pane.desk.sounding();
        if ui
            .button(if sounding { "playing…" } else { "play" })
            .on_hover_text("render the program and hand the wave to the system player")
            .clicked()
        {
            pane.desk.play();
        }
        if ui.button("stop").clicked() {
            pane.desk.stop();
        }
        ui.checkbox(&mut pane.looping, "loop")
            .on_hover_text("start again as soon as the piece ends");
        if full {
            ui.separator();
            if ui
                .button("re-read")
                .on_hover_text("take the program as it stands now")
                .clicked()
            {
                reaction.reread = true;
            }
            if ui.button("export wav").clicked() {
                reaction.export = true;
            }
        }
    });

    // A loop is a restart when the player is gone, checked here because this is
    // the one place that runs every frame.
    if pane.looping && !pane.desk.sounding() && pane.desk.loaded() {
        pane.desk.play();
    }

    ui.add_space(4.0);
    let mut retune = false;
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(k.secondary, "ROOT");
        retune |= ui
            .add(
                egui::DragValue::new(&mut pane.desk.tuning.root_hz)
                    .speed(1.0)
                    .range(40.0..=880.0)
                    .suffix(" Hz"),
            )
            .on_hover_text("the frequency the first degree of the Fibonacci scale sits on")
            .changed();
        ui.colored_label(k.secondary, "PACE");
        retune |= ui
            .add(
                egui::DragValue::new(&mut pane.desk.tuning.pace)
                    .speed(0.005)
                    .range(0.03..=1.0)
                    .suffix(" s/atom"),
            )
            .on_hover_text("seconds one atom is given; the piece is this times its atoms")
            .changed();
        ui.colored_label(k.secondary, "PARTIALS");
        retune |= ui
            .add(egui::DragValue::new(&mut pane.desk.tuning.partials).range(1..=6))
            .on_hover_text("how many Zeckendorf terms are allowed to sound")
            .changed();
        ui.colored_label(k.secondary, "OCTAVES");
        retune |= ui
            .add(egui::DragValue::new(&mut pane.desk.tuning.octaves).range(1..=6))
            .on_hover_text("how many octaves of indentation before depth wraps")
            .changed();
        if ui
            .button(pane.desk.tuning.timbre.name())
            .on_hover_text("the carrier under the partials")
            .clicked()
        {
            pane.desk.tuning.timbre = pane.desk.tuning.timbre.next();
            retune = true;
        }
        // Cycling with a single button is fine for four shapes; a menu of four
        // would be a menu for its own sake.
        let _ = Timbre::ALL;
    });
    if retune {
        pane.desk.retune();
    }

    ui.add_space(6.0);
    wave(ui, pane, k);
    ui.add_space(4.0);
    ui.colored_label(k.faint, &pane.desk.message);
    reading(ui, pane, k);
    ui.add_space(6.0);
    ui.separator();
    score(ui, pane, k);
    reaction
}

/// What the tab wants the editor to do after drawing.
#[derive(Default)]
pub(crate) struct Reaction {
    /// Take the current program from wherever this tab was opened.
    pub(crate) reread: bool,
    /// Ask for a path and write the wave.
    pub(crate) export: bool,
}

/// The wave itself, with the notes marked under it and the playhead over it.
fn wave(ui: &mut egui::Ui, pane: &mut MusicPane, k: &Ink) {
    let width = ui.available_width().max(64.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, WAVE_HEIGHT), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, k.fill);
    let middle = rect.center().y;
    painter.line_segment(
        [
            Pos2::new(rect.left(), middle),
            Pos2::new(rect.right(), middle),
        ],
        Stroke::new(1.0, k.rule),
    );
    if !pane.desk.loaded() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no program loaded",
            egui::FontId::proportional(13.0),
            k.faint,
        );
        return;
    }

    // One column per point of width: the wave is drawn from the samples that
    // fall in that column, not from a sample picked out of it, so what is on
    // screen is the shape of the file rather than an alias of it.
    let columns = width.floor().max(1.0) as usize;
    let bands = pane.desk.bands(columns);
    let half = (WAVE_HEIGHT * 0.5) - 6.0;
    for (column, band) in bands.iter().enumerate() {
        let x = rect.left() + column as f32 + 0.5;
        let top = middle - band.high * half;
        let bottom = middle - band.low * half;
        // The body of the wave in the accent, its loud core brighter: the
        // second stroke is the RMS, so the eye reads density as well as peak.
        painter.line_segment(
            [Pos2::new(x, top), Pos2::new(x, bottom)],
            Stroke::new(1.0, k.secondary),
        );
        let core = band.rms * half;
        painter.line_segment(
            [
                Pos2::new(x, middle - core),
                Pos2::new(x, middle + core),
            ],
            Stroke::new(1.0, k.accent),
        );
    }

    // Every note as a tick along the foot, its height its depth: the drawing's
    // indentation, drawn as the piece's own structure.
    let seconds = pane.desk.score.seconds.max(f64::EPSILON);
    let depth = pane.desk.score.depth.max(1) as f32;
    for note in &pane.desk.score.notes {
        let x = rect.left() + (note.start / seconds) as f32 * width;
        let height = 4.0 + (note.depth as f32 / depth) * 10.0;
        painter.line_segment(
            [
                Pos2::new(x, rect.bottom()),
                Pos2::new(x, rect.bottom() - height),
            ],
            Stroke::new(1.0, k.faint),
        );
    }

    // The playhead, from the clock. Nothing here owns the audio device, so this
    // is where the player has got to and not where the speaker is.
    if let Some(at) = pane.desk.playhead() {
        let x = rect.left() + (at / seconds).clamp(0.0, 1.0) as f32 * width;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.5, k.danger),
        );
        ui.ctx().request_repaint();
    }

    // Hovering the wave picks the note sounding at that instant, which is what
    // the reading below is written for.
    if let Some(pointer) = response.hover_pos() {
        let at = ((pointer.x - rect.left()) / width).clamp(0.0, 1.0) as f64 * seconds;
        pane.reading = pane
            .desk
            .score
            .notes
            .iter()
            .position(|note| at >= note.start && at < note.start + note.duration);
        if let Some(index) = pane.reading {
            let note = &pane.desk.score.notes[index];
            let x = rect.left() + (note.start / seconds) as f32 * width;
            let w = (note.duration / seconds) as f32 * width;
            painter.rect_stroke(
                Rect::from_min_size(
                    Pos2::new(x, rect.top() + 2.0),
                    Vec2::new(w.max(1.5), rect.height() - 4.0),
                ),
                1.0,
                Stroke::new(1.0, k.accent),
            );
        }
    }
}

/// The arithmetic of one note, written out.
///
/// This is the part that earns the claim in the module docs. A tone is not
/// asserted to come from the Fibonacci sequence — the word, the number, the
/// terms it decomposes into and the degree they choose are all on screen
/// together, so the reader can check it.
fn reading(ui: &mut egui::Ui, pane: &MusicPane, k: &Ink) {
    let Some(note) = pane.reading.and_then(|index| pane.desk.score.notes.get(index)) else {
        ui.colored_label(k.faint, "hover the wave to read a tone");
        return;
    };
    let reading = pane.desk.reading(&note.token);
    let terms = reading
        .terms
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(" + ");
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(k.accent, format!("“{}”", note.token));
        ui.colored_label(k.faint, "=");
        ui.colored_label(k.ink, reading.value.to_string());
        ui.colored_label(k.faint, "=");
        ui.colored_label(k.ink, terms);
        ui.colored_label(k.faint, "→ degree");
        ui.colored_label(k.ink, reading.degree.to_string());
        ui.colored_label(k.faint, "·");
        ui.colored_label(k.ink, format!("{:.1} Hz", note.hz));
        ui.colored_label(k.faint, "· depth");
        ui.colored_label(k.ink, note.depth.to_string());
        ui.colored_label(k.faint, "· partials");
        ui.colored_label(
            k.ink,
            note.partials
                .iter()
                .map(|(harmonic, _)| format!("{harmonic:.0}"))
                .collect::<Vec<_>>()
                .join(":"),
        );
    });
}

/// The score as a list, in the order it sounds.
fn score(ui: &mut egui::Ui, pane: &MusicPane, k: &Ink) {
    ui.colored_label(
        k.mid,
        "SCORE · onset · length · depth · pitch · what sounded",
    );
    egui::ScrollArea::vertical()
        .id_salt("music_score")
        .max_height(260.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, note) in pane.desk.score.notes.iter().enumerate() {
                let selected = pane.reading == Some(index);
                ui.horizontal(|ui| {
                    let tone = if selected { k.accent } else { k.faint };
                    ui.colored_label(tone, format!("{:>7.3}s", note.start));
                    ui.colored_label(tone, format!("{:>6.3}s", note.duration));
                    ui.colored_label(depth_tone(note, k), format!("d{}", note.depth));
                    ui.colored_label(tone, format!("{:>8.1} Hz", note.hz));
                    ui.colored_label(if selected { k.accent } else { k.ink }, &note.token);
                });
            }
        });
}

/// Depth is drawn as ink strength: the shallow structure is the loud, dark part
/// of the piece and the fine detail recedes, exactly as it sounds.
fn depth_tone(note: &Note, k: &Ink) -> Color32 {
    if note.depth == 0 {
        k.ink
    } else if note.depth < 3 {
        k.secondary
    } else {
        k.faint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every string and every line the tab painted, in one frame.
    fn painted(pane: &mut MusicPane) -> (String, usize) {
        painted_with(pane, true)
    }

    fn painted_with(pane: &mut MusicPane, full: bool) -> (String, usize) {
        fn harvest(shape: &egui::epaint::Shape, text: &mut String, lines: &mut usize) {
            match shape {
                egui::epaint::Shape::Text(drawn) => {
                    text.push_str(drawn.galley.text());
                    text.push('\n');
                }
                egui::epaint::Shape::LineSegment { .. } => *lines += 1,
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        harvest(shape, text, lines);
                    }
                }
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 900.0),
            )),
            ..Default::default()
        };
        let ink = Ink::load();
        let (mut text, mut lines) = (String::new(), 0);
        // Twice: egui sizes a panel from the previous frame, so the first pass
        // is a measurement and the second is the drawing.
        for pass in 0..2 {
            let output = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    tab(ui, pane, &ink, full);
                });
            });
            if pass == 1 {
                for shape in &output.shapes {
                    harvest(&shape.shape, &mut text, &mut lines);
                }
            }
        }
        (text, lines)
    }

    #[test]
    fn the_sound_tab_draws_a_wave_and_names_what_it_is_playing() {
        let mut pane = MusicPane::from_source(
            "(\"joy and expansion\" x)",
            "drawing".to_string(),
            None,
        )
        .expect("the fixture parses");
        let (text, lines) = painted(&mut pane);

        // The score, word by word — the atomic level is what the tab is for.
        for word in ["joy", "and", "expansion"] {
            assert!(text.contains(word), "{word} is not in the tab: {text}");
        }
        // The controls, so the synth can actually be tuned from here.
        for control in ["play", "stop", "re-read", "export wav", "sine", "ROOT"] {
            assert!(text.contains(control), "no {control} control: {text}");
        }
        // And the wave: one line segment per column of it, so a few hundred at
        // this width. A tab that drew the numbers and not the shape would pass
        // every assertion above.
        assert!(
            lines > 200,
            "the wave was not drawn — only {lines} line segments"
        );
    }

    #[test]
    fn a_torn_off_window_leaves_out_the_controls_it_cannot_honour() {
        // Re-reading takes the program from another tab and exporting opens a
        // file dialog; a window running on its own clock can do neither. They
        // are absent rather than dead — a button that does nothing is worse
        // than a button that is not there.
        let mut pane =
            MusicPane::from_source("(\"joy\" x)", "drawing".to_string(), None).expect("parses");
        let full = painted_with(&mut pane, true).0;
        let torn = painted_with(&mut pane, false).0;
        for control in ["re-read", "export wav"] {
            assert!(full.contains(control), "the full tab lost {control}");
            assert!(!torn.contains(control), "the torn window still offers {control}");
        }
        // Everything the view is actually for is still there.
        for control in ["play", "stop", "ROOT", "joy"] {
            assert!(torn.contains(control), "the torn window lost {control}");
        }
    }

    #[test]
    fn a_program_that_does_not_parse_opens_no_tab() {
        assert!(MusicPane::from_source("(", "drawing".to_string(), None).is_err());
    }
}
