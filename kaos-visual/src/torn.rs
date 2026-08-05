//! Tabs pulled out of the window into windows of their own.
//!
//! A tab is a view of something, and some of those things are worth watching
//! *while* you work on the drawing that produced them — a generation stepping,
//! a piece playing. Inside one window they are exclusive: looking at the
//! automaton means not looking at the mandala. Torn off, they are not.
//!
//! **They run on their own clock.** egui offers two kinds of extra window, and
//! the difference decides the feature. An *immediate* viewport is drawn inside
//! its parent's frame, so it repaints when the parent does and freezes whenever
//! the parent is busy — a generation in one of those would step only while the
//! main window was being used. A *deferred* viewport has its own repaint loop.
//! That is what a torn tab gets, and it is why one can animate beside an idle
//! editor.
//!
//! The cost of that independence is the whole shape of this module. A deferred
//! viewport's paint callback must be `Send + Sync + 'static`: it cannot borrow
//! the editor, because it does not run inside the editor's frame. So a torn
//! pane's state moves behind an [`Arc<Mutex<…>>`] and the window draws from
//! that alone — which in turn is why only panes that already draw as free
//! functions of their own state can be torn off. That is not a limitation
//! being worked around; it is the same condition stated twice. A pane that
//! needs the editor to draw itself is a pane that is not independent, and
//! putting it in a window of its own would only make the coupling harder to
//! see.
//!
//! Minimal chrome by design: the torn window carries what that view needs and
//! nothing else. No tab bar, no palette, no side panel — those belong to the
//! window that owns the documents.

use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::automata;
use crate::music::MusicPane;
use crate::theme::Ink;

/// A pane that can live in a window of its own.
///
/// One variant per tab kind that draws from its own state. Adding a kind is
/// adding a variant and an arm in [`Torn::show`] — and finding that a kind
/// cannot be added is finding that it was never self-contained.
pub(crate) enum TornPane {
    /// A program as sound: score, samples, and the player, all in the desk.
    Music(Box<MusicPane>),
    /// A run as the automaton it computes. It carries its own lattice and
    /// steps on wall-clock time, so it animates in its own window.
    Generation(Box<automata::Automaton>, String),
}

impl TornPane {
    /// The window title — what the tab was called.
    pub(crate) fn title(&self) -> String {
        match self {
            TornPane::Music(pane) => format!("sound · {}", pane.origin),
            TornPane::Generation(_, origin) => format!("generation · {origin}"),
        }
    }
}

/// One torn-off window.
pub(crate) struct Torn {
    /// Stable across frames: egui identifies a viewport by this, and a new id
    /// each frame would close and reopen the window every time.
    id: egui::ViewportId,
    title: String,
    /// Shared with the paint callback, which runs off the editor's frame.
    pane: Arc<Mutex<TornPane>>,
    /// Set by the window when it is closed, read by the editor afterwards.
    closing: Arc<std::sync::atomic::AtomicBool>,
}

impl Torn {
    /// Tear a pane out into its own window.
    ///
    /// `sequence` only has to be unique within this session; it is what keeps
    /// two windows of the same kind apart.
    pub(crate) fn new(pane: TornPane, sequence: u64) -> Self {
        let title = pane.title();
        Self {
            id: egui::ViewportId::from_hash_of(("kaos-torn", sequence)),
            title,
            pane: Arc::new(Mutex::new(pane)),
            closing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Whether the user has closed this window, so the editor can drop it.
    pub(crate) fn closed(&self) -> bool {
        self.closing.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Take the pane back — for putting a closed window's tab back on the bar.
    ///
    /// Returns `None` only if the paint callback still holds the lock, which it
    /// does not once the viewport is gone.
    pub(crate) fn reclaim(self) -> Option<TornPane> {
        Arc::try_unwrap(self.pane).ok().and_then(|pane| pane.into_inner().ok())
    }

    /// Draw the window. Called once per editor frame; the window itself
    /// repaints on its own schedule.
    pub(crate) fn show(&self, ctx: &egui::Context, ink: Ink) {
        let pane = Arc::clone(&self.pane);
        let closing = Arc::clone(&self.closing);
        let title = self.title.clone();
        ctx.show_viewport_deferred(
            self.id,
            egui::ViewportBuilder::default()
                .with_title(&title)
                .with_inner_size([720.0, 560.0]),
            move |ctx, _class| {
                // A window the user closed is remembered rather than acted on
                // here: the editor owns the tab list, and this callback is not
                // running inside its frame.
                if ctx.input(|input| input.viewport().close_requested()) {
                    closing.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    // A poisoned lock means a previous paint panicked. Showing
                    // the pane anyway is better than propagating the panic into
                    // every later frame of a window the user can still close.
                    let mut pane = match pane.lock() {
                        Ok(pane) => pane,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    match &mut *pane {
                        TornPane::Music(pane) => {
                            // The reactions a torn window cannot honour are the
                            // ones that need the editor — re-reading from
                            // another tab, and the file dialog. The controls for
                            // them are absent rather than dead: see
                            // `music::tab`'s `full` flag.
                            crate::music::tab(ui, pane, &ink, false);
                        }
                        TornPane::Generation(machine, origin) => {
                            ui.horizontal(|ui| {
                                ui.colored_label(ink.mid, "GENERATION ·");
                                ui.colored_label(ink.ink, origin.as_str());
                                ui.separator();
                                if ui.button("step").clicked() {
                                    machine.step();
                                }
                                ui.colored_label(ink.faint, machine.generation.to_string());
                            });
                            ui.add_space(6.0);
                            crate::draw_generation(ui, machine, ink);
                        }
                    }
                });
            },
        );
    }
}
