//! Tabs pulled out of the window into windows of their own.
//!
//! A tab is a view of something, and some of those things are worth watching
//! *while* you work on the drawing that produced them. Inside one window they
//! are exclusive: looking at the source means not looking at the mandala.
//! Torn off, they are not.
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
//! the main editor, because it does not run inside the main editor's frame. So
//! a torn pane owns a detached editor behind an [`Arc<Mutex<…>>`], and that
//! editor draws the pane from its own state.
//!
//! Minimal chrome by design: the torn window carries what that view needs and
//! nothing else. No tab bar, no palette, no side panel — those belong to the
//! window that owns the documents.

use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::theme::Ink;

/// A detached editor owns the moved pane and all state needed by its renderer.
/// Keeping one variant here means adding a tab kind does not require another
/// special-case window implementation.
pub(crate) enum TornPane {
    Editor(Box<crate::Editor>),
}

impl TornPane {
    /// The window title — what the tab was called.
    pub(crate) fn title(&self) -> String {
        match self {
            TornPane::Editor(editor) => editor.detached_title(),
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
        Arc::try_unwrap(self.pane)
            .ok()
            .and_then(|pane| pane.into_inner().ok())
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
                        TornPane::Editor(editor) => editor.show_detached(ui, ink),
                    }
                });
            },
        );
    }
}
