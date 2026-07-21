//! The `kaos visual` mandala editor: a native egui window over [`kaos_core::visual`].
//!
//! Deliberately thin. Every rule about what a drawing *means* — the forms,
//! their arity, code generation, loading, hit-testing, the sigil geometry and
//! the viewport math — lives in [`kaos_core::visual`] and is tested without a
//! window. This file paints those shapes and routes pointer events.
//!
//! Rendering is native (egui on glow), not a webview, so the editor needs no
//! system libraries beyond the OpenGL and windowing ones any desktop already
//! has.
//!
//! It is its own application. `kaos-visual [program-or-file]` runs the editor
//! with no terminal app involved; the `kaos visual` subcommand is a second
//! front door onto the same [`open`] and [`run`] pair.

use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke as UiStroke, Vec2};

use kaos_core::tabs::{TabId, Tabs};
use kaos_core::visual::{Form, Mandala, NodeId, Shape, Stroke, View, NODE_R, NODE_RY};

/// One open drawing. Each tab keeps its own canvas *and its own viewport and
/// selection*, so switching tabs returns you to exactly where you were.
#[derive(Default)]
struct Doc {
    mandala: Mandala,
    view: View,
    pending: Option<NodeId>,
    selected: Option<NodeId>,
    /// Nodes currently being evaluated. Driven by a real run on a background
    /// thread, so the ring means work is actually in flight rather than
    /// standing in for it.
    running: std::collections::HashSet<NodeId>,
    /// The editable source, kept in step with the drawing in both directions.
    text: String,
    /// What the drawing last generated. Comparing against it is how we tell an
    /// edit made on the canvas from one typed into the panel, without either
    /// overwriting the other mid-keystroke.
    generated: String,
}

/// A conversation, with the same durable sessions the terminal app writes.
/// Opening one here and resuming it there is the same store and the same
/// format — the transcript is not a second, parallel notion of a chat.
struct ChatPane {
    session: kaos_core::sessions::Session,
    input: String,
    /// Showing the session list rather than a transcript.
    browsing: bool,
}

impl Default for ChatPane {
    fn default() -> Self {
        Self {
            session: kaos_core::sessions::Session::new(
                kaos_core::config::value("KAOS_MODEL").unwrap_or_else(|| "sim".into()),
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
            input: String::new(),
            browsing: true,
        }
    }
}

/// What a tab holds. The tab machinery is generic, so adding a kind is adding
/// a variant here rather than another parallel list.
enum Pane {
    Mandala(Doc),
    Chat(ChatPane),
    /// The personal sigil library — the same `~/.kaos/sigils` the terminal
    /// explorer browses. Opening one draws it on a new canvas.
    Sigils {
        query: String,
        notice: Option<String>,
    },
    /// Rebis source, as text. The same buffer the terminal workspace edits and
    /// the same library it saves to, checked with the same parser.
    Source {
        name: String,
        text: String,
        notice: Option<String>,
    },
}

// ── palette ─────────────────────────────────────────────────────────────────
//
// Monochrome, from `theme.rs`, so the editor and the terminal app wear the same
// mode. `/theme dark|light` persists the choice for both.

fn rgb((r, g, b): (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The five tones of the current mode, resolved once per window.
#[derive(Clone, Copy)]
struct Ink {
    accent: Color32,
    ground: Color32,
    chrome: Color32,
    fill: Color32,
    ink: Color32,
    faint: Color32,
}

impl Ink {
    fn load() -> Self {
        let p = kaos_core::theme::current();
        Self {
            accent: rgb(p.accent),
            ground: rgb(p.ground),
            chrome: rgb(p.chrome),
            fill: rgb(p.fill),
            ink: rgb(p.ink),
            faint: rgb(p.faint),
        }
    }
}

/// What a click does.
#[derive(Clone, PartialEq)]
enum Tool {
    /// Place this form.
    Place(Form),
    /// Click two shapes to link them with a flow node (`->` or `<-`). The node
    /// is created behind the arrow, so drawing the arrow *is* writing the form.
    Flow(Form),
    /// Click a parent then a child to attach it directly, for the forms that
    /// take a list — `[]`, `( )`, `$`, a call, a quote, a macro body.
    Child,
    /// Click a shape to select it.
    Select,
}

/// An in-progress pointer gesture.
#[derive(Clone, Copy, PartialEq)]
enum Drag {
    None,
    /// Moving a shape. `grab` is the offset from the shape's centre, in world
    /// units, so the shape does not jump to the cursor.
    Node {
        id: NodeId,
        grab: (f64, f64),
    },
    /// Panning the canvas.
    Pan,
}

/// Resolve what an argument names into a drawing to open.
///
/// Same convention as `rebis run`: a readable path loads, anything else is
/// treated as inline Rebis source, and nothing at all is an empty canvas.
/// Kept here rather than in a caller so every way of starting the editor
/// agrees about what its argument means.
pub fn open(arg: &str) -> Result<Mandala, String> {
    let arg = arg.trim();
    if arg.is_empty() {
        return Ok(Mandala::new());
    }
    let source = std::fs::read_to_string(arg).unwrap_or_else(|_| arg.to_string());
    Mandala::from_rebis(&source).map_err(|e| e.to_string())
}

/// Open the editor window on `start`. Blocks until the window closes.
pub fn run(start: Mandala) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title("kaos visual — mandala editor"),
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "kaos visual",
        options,
        Box::new(|cc| {
            let editor = Editor::new(start);
            install_theme(&cc.egui_ctx, editor.ink);
            Ok(Box::new(editor))
        }),
    ) {
        eprintln!("visual: {error}");
    }
}

/// Dress egui in the kaos palette so the editor matches the terminal app.
fn install_theme(ctx: &egui::Context, k: Ink) {
    let mut visuals = if kaos_core::theme::mode() == kaos_core::theme::Mode::Light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.panel_fill = k.chrome;
    visuals.window_fill = k.chrome;
    visuals.extreme_bg_color = k.ground;
    visuals.override_text_color = Some(k.ink);
    visuals.widgets.noninteractive.bg_stroke = UiStroke::new(1.0, k.faint);
    visuals.widgets.inactive.bg_fill = k.fill;
    visuals.widgets.inactive.bg_stroke = UiStroke::new(1.0, k.faint);
    visuals.widgets.hovered.bg_stroke = UiStroke::new(1.0, k.accent);
    visuals.widgets.active.bg_fill = k.accent;
    visuals.selection.bg_fill = k.accent.gamma_multiply(0.35);
    visuals.selection.stroke = UiStroke::new(1.0, k.accent);
    // egui's defaults still carry colour in a few corners — selection blue,
    // hyperlink blue, amber warnings, red errors. Nothing here is allowed to
    // be anything but grey.
    visuals.hyperlink_color = k.ink;
    visuals.warn_fg_color = k.ink;
    visuals.error_fg_color = k.ink;
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.fg_stroke = UiStroke::new(w.fg_stroke.width, k.ink);
    }
    visuals.widgets.hovered.weak_bg_fill = k.fill;
    visuals.widgets.active.weak_bg_fill = k.faint;
    ctx.set_visuals(visuals);
}

struct Editor {
    ink: Ink,
    /// Where kaos was started. Runs, relative reads, imports and output paths
    /// all resolve from here, exactly as in the terminal app, so a program
    /// drawn here means the same thing when it is run.
    cwd: std::path::PathBuf,
    tabs: Tabs<Pane>,
    /// Stands in while a chat tab is active, so the canvas code never has to
    /// ask whether there is a drawing. It is never drawn.
    scratch: Doc,
    /// The tool is deliberately shared across tabs: it is a mode of working,
    /// not a property of a drawing.
    tool: Tool,
    drag: Drag,
    notice: Option<String>,
    /// Evidence a run resolves against, and the last answer. Held on the
    /// editor rather than a tab: the record is the context you are working in,
    /// not a property of one drawing.
    record: String,
    outcome: Option<String>,
    /// The in-flight run's answer, once its thread finishes.
    pending_run: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
}

impl Editor {
    fn new(mandala: Mandala) -> Self {
        let mut tabs = Tabs::new();
        tabs.open(
            "mandala",
            Pane::Mandala(Doc {
                mandala,
                ..Doc::default()
            }),
        );
        Self {
            ink: Ink::load(),
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            tabs,
            scratch: Doc::default(),
            tool: Tool::Place(Form::Prompt),
            drag: Drag::None,
            notice: None,
            record: String::new(),
            outcome: None,
            pending_run: None,
        }
    }

    /// Run `source` deterministically and keep the answer for display.
    ///
    /// This is the offline calculus, not a model call — it needs no provider
    /// and no child process, which is why the editor can offer it while
    /// standing alone.
    fn run_source(&mut self, source: &str) {
        let record = kaos_core::runs::record_from_text(&self.record);
        let source = source.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        // Off the UI thread, so the ring is driven by work genuinely in
        // flight. The deterministic run is quick; a model-backed one will not
        // be, and this is the seam it will arrive through.
        let linger = std::env::var("KAOS_VISUAL_RUN_LINGER_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());
        std::thread::spawn(move || {
            // A hook for watching the ring turn; unset in normal use.
            if let Some(ms) = linger {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            let answer = kaos_core::runs::evaluate(&source, &record)
                .map(|o| o.to_string())
                .map_err(|e| e);
            let _ = tx.send(answer);
        });
        self.pending_run = Some(rx);
        self.outcome = Some("running…".to_string());
        let ids: std::collections::HashSet<NodeId> =
            self.doc().mandala.nodes().iter().map(|n| n.id).collect();
        self.doc_mut().running = ids;
    }

    /// Collect a finished run and stop the rings.
    fn poll_run(&mut self) {
        let done = match &self.pending_run {
            Some(rx) => match rx.try_recv() {
                Ok(answer) => Some(answer),
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("the run ended without an answer".to_string()))
                }
            },
            None => return,
        };
        self.pending_run = None;
        self.doc_mut().running.clear();
        if let Some(answer) = done {
            self.outcome = Some(match answer {
                Ok(text) => text,
                Err(e) => e,
            });
        }
    }

    /// The open drawing, or a stand-in while a conversation is on screen — so
    /// the canvas code never has to ask which kind of tab is active.
    fn doc(&self) -> &Doc {
        match self.tabs.active() {
            Some(Pane::Mandala(d)) => d,
            _ => &self.scratch,
        }
    }

    fn doc_mut(&mut self) -> &mut Doc {
        match self.tabs.active_mut() {
            Some(Pane::Mandala(d)) => d,
            _ => &mut self.scratch,
        }
    }

    /// Whether a drawing is on screen, as opposed to a conversation.
    fn on_mandala(&self) -> bool {
        matches!(self.tabs.active(), Some(Pane::Mandala(_)))
    }
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_keys(ctx);
        self.poll_run();
        if self.on_mandala() {
            self.sync();
        }
        self.header(ctx);
        self.tab_bar(ctx);
        if self.on_mandala() {
            self.palette(ctx);
            self.side(ctx);
        }
        self.footer(ctx);
        self.runs(ctx);
        let on_mandala = self.on_mandala();
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(self.ink.ground))
            .show(ctx, |ui| match self.tabs.active() {
                Some(Pane::Mandala(_)) => self.canvas(ui),
                Some(Pane::Chat(_)) => self.chat(ui),
                Some(Pane::Source { .. }) => self.source(ui),
                _ => self.sigils(ui),
            });
    }
}

impl Editor {
    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(self.ink.accent, "KAOS VISUAL");
                ui.separator();
                ui.colored_label(
                    self.ink.faint,
                    format!("{}%", (self.doc_mut().view.zoom * 100.0).round()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("clear").clicked() {
                        self.doc_mut().mandala = Mandala::new();
                        self.doc_mut().pending = None;
                        self.doc_mut().selected = None;
                    }
                    if ui.button("edit as text").clicked() {
                        if let Ok(src) = self.doc().mandala.to_rebis() {
                            self.tabs.open(
                                "source",
                                Pane::Source {
                                    name: String::new(),
                                    text: src,
                                    notice: None,
                                },
                            );
                        }
                    }
                    if ui.button("reset view").clicked() {
                        self.doc_mut().view = View::new();
                    }
                    let built = self.doc_mut().mandala.to_rebis();
                    // Only offer the hand-off when there is source to hand over.
                    ui.add_enabled_ui(built.is_ok(), |ui| {
                        if ui.button("open in terminal").clicked() {
                            match &built {
                                Ok(src) => {
                                    self.notice = Some(match open_in_terminal(src, &self.cwd) {
                                        Ok(()) => "opened in terminal".to_string(),
                                        Err(e) => e,
                                    })
                                }
                                Err(e) => self.notice = Some(e.to_string()),
                            }
                        }
                    });
                    if let Some(note) = &self.notice {
                        ui.colored_label(self.ink.faint, note);
                    }
                });
            });
        });
    }

    /// The open drawings. Each keeps its own canvas, viewport and selection,
    /// so switching back returns you exactly where you were.
    fn tab_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let active = self.tabs.active_id();
                let mut select: Option<TabId> = None;
                let mut close: Option<TabId> = None;
                for tab in self.tabs.iter() {
                    let on = Some(tab.id) == active;
                    if ui.selectable_label(on, &tab.title).clicked() {
                        select = Some(tab.id);
                    }
                    // Only the active tab offers its close button, so the bar
                    // stays quiet and a stray click cannot shut the wrong one.
                    if on && self.tabs.len() > 1 && ui.small_button("×").clicked() {
                        close = Some(tab.id);
                    }
                    ui.separator();
                }
                if ui.small_button("+ mandala").clicked() {
                    let n = self.tabs.len() + 1;
                    self.tabs
                        .open(format!("mandala {n}"), Pane::Mandala(Doc::default()));
                }
                if ui.small_button("+ chat").clicked() {
                    self.tabs.open("chat", Pane::Chat(ChatPane::default()));
                }
                if ui.small_button("+ source").clicked() {
                    self.tabs.open(
                        "source",
                        Pane::Source {
                            name: String::new(),
                            text: String::new(),
                            notice: None,
                        },
                    );
                }
                if ui.small_button("+ sigils").clicked() {
                    self.tabs.open(
                        "sigils",
                        Pane::Sigils {
                            query: String::new(),
                            notice: None,
                        },
                    );
                }
                if let Some(id) = select {
                    self.tabs.select(id);
                }
                if let Some(id) = close {
                    self.tabs.close(id);
                }
            });
        });
    }

    /// A conversation tab: browse the saved sessions, or read and extend one.
    ///
    /// These are the same sessions `/resume` reads in the terminal app — same
    /// store, same format — so a conversation started in either interface can
    /// be picked up in the other.
    fn chat(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let Some(Pane::Chat(chat)) = self.tabs.active_mut() else {
            return;
        };

        if chat.browsing {
            let store = kaos_core::sessions::Store::default_store();
            let list = store.list();
            ui.add_space(8.0);
            ui.colored_label(k.faint, "SESSIONS");
            if list.is_empty() {
                ui.colored_label(k.faint, "none saved yet — start typing below");
            }
            let mut resume = None;
            egui::ScrollArea::vertical()
                .max_height(ui.available_height() - 90.0)
                .show(ui, |ui| {
                    for s in &list {
                        let line = format!("{:>3} turns   {}", s.turns, s.title);
                        if ui.selectable_label(false, line).clicked() {
                            resume = Some(s.id.clone());
                        }
                    }
                });
            if let Some(id) = resume {
                if let Ok(loaded) = store.load(&id) {
                    chat.session = loaded;
                    chat.browsing = false;
                }
            }
            if ui.button("new conversation").clicked() {
                chat.browsing = false;
            }
        } else {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.colored_label(k.faint, chat.session.title());
                if ui.small_button("sessions").clicked() {
                    chat.browsing = true;
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(ui.available_height() - 60.0)
                .show(ui, |ui| {
                    for turn in &chat.session.turns {
                        let (who, tone) = match turn.role {
                            kaos_core::sessions::Role::User => ("you", k.ink),
                            kaos_core::sessions::Role::Model => ("model", k.faint),
                        };
                        ui.horizontal_top(|ui| {
                            ui.colored_label(tone, format!("{who:<6}"));
                            ui.add(egui::Label::new(egui::RichText::new(&turn.text)).wrap());
                        });
                    }
                });
        }

        ui.separator();
        ui.horizontal(|ui| {
            let send = ui
                .add(
                    egui::TextEdit::singleline(&mut chat.input)
                        .desired_width(ui.available_width() - 70.0)
                        .hint_text("say something"),
                )
                .lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (send || ui.button("send").clicked()) && !chat.input.trim().is_empty() {
                chat.browsing = false;
                let said = std::mem::take(&mut chat.input);
                chat.session.push(kaos_core::sessions::Role::User, said);
                // Persist immediately: the terminal app saves on every turn for
                // the same reason, so a crash loses nothing already said.
                let _ = kaos_core::sessions::Store::default_store().save(&chat.session);
            }
        });
    }

    /// The sigil library. Opening one parses it and lays it out as a drawing,
    /// so a saved program becomes a mandala without a round trip through text.
    fn sigils(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let Some(Pane::Sigils { query, notice }) = self.tabs.active_mut() else {
            return;
        };
        let mut open: Option<String> = None;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.colored_label(k.faint, "SIGILS");
            ui.add(
                egui::TextEdit::singleline(query)
                    .desired_width(220.0)
                    .hint_text("search"),
            );
        });
        if let Some(note) = notice.clone() {
            ui.colored_label(k.faint, note);
        }
        ui.separator();
        let lib = kaos_core::sigils::Library::default_library();
        let found = lib.search(query);
        if found.is_empty() {
            ui.colored_label(k.faint, "nothing saved yet");
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for e in &found {
                if ui
                    .selectable_label(false, format!("{}   {} bytes", e.name, e.bytes))
                    .clicked()
                {
                    open = Some(e.name.clone());
                }
            }
        });

        if let Some(name) = open {
            match lib.load(&name) {
                Ok(source) => match Mandala::from_rebis(&source) {
                    Ok(mandala) => {
                        self.tabs.open(
                            name,
                            Pane::Mandala(Doc {
                                mandala,
                                ..Doc::default()
                            }),
                        );
                    }
                    Err(e) => {
                        if let Some(Pane::Sigils { notice, .. }) = self.tabs.active_mut() {
                            *notice = Some(format!("{name}: {e}"));
                        }
                    }
                },
                Err(e) => {
                    if let Some(Pane::Sigils { notice, .. }) = self.tabs.active_mut() {
                        *notice = Some(format!("{name}: {e}"));
                    }
                }
            }
        }
    }

    /// A source tab: Rebis as text, checked as you type.
    ///
    /// Validation, saving and drawing all go through the same code the
    /// terminal app uses — `rebis_lang::parse`, `sigils::Library`, and
    /// `Mandala::from_rebis` — so a program means one thing in both.
    fn source(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let mut draw: Option<String> = None;
        let mut save: Option<(String, String)> = None;
        let mut run: Option<String> = None;
        let Some(Pane::Source { name, text, notice }) = self.tabs.active_mut() else {
            return;
        };

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.colored_label(k.faint, "NAME");
            ui.add(
                egui::TextEdit::singleline(name)
                    .desired_width(240.0)
                    .hint_text("team/reviews"),
            );
            if ui.button("save").clicked() {
                save = Some((name.clone(), text.clone()));
            }
            if ui.button("draw").clicked() {
                draw = Some(text.clone());
            }
            if ui.button("run").clicked() {
                run = Some(text.clone());
            }
        });

        // The live diagnostic, exactly as the workspace shows it.
        let status = match rebis_lang::parse(text) {
            _ if text.trim().is_empty() => String::new(),
            Ok(_) => "valid".to_string(),
            Err(e) => e.to_string(),
        };
        if let Some(note) = notice.clone() {
            ui.colored_label(k.faint, note);
        }
        ui.colored_label(k.faint, status);
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(28),
            );
        });

        if let Some(text) = run {
            self.run_source(&text);
        }
        if let Some((name, text)) = save {
            let result = kaos_core::sigils::Library::default_library().save(&name, &text);
            if let Some(Pane::Source { notice, .. }) = self.tabs.active_mut() {
                *notice = Some(match result {
                    Ok(p) => format!("saved {}", p.display()),
                    Err(e) => e.to_string(),
                });
            }
        }
        if let Some(text) = draw {
            match Mandala::from_rebis(&text) {
                Ok(mandala) => {
                    self.tabs.open(
                        "drawn",
                        Pane::Mandala(Doc {
                            mandala,
                            ..Doc::default()
                        }),
                    );
                }
                Err(e) => {
                    if let Some(Pane::Source { notice, .. }) = self.tabs.active_mut() {
                        *notice = Some(e.to_string());
                    }
                }
            }
        }
    }

    fn palette(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("palette")
            .exact_width(180.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.colored_label(self.ink.faint, "FORMS");
                for (label, make, _) in Form::ALL {
                    let form = make();
                    let on = self.tool == Tool::Place(form.clone());
                    if ui.selectable_label(on, *label).clicked() {
                        self.tool = Tool::Place(form);
                        self.doc_mut().pending = None;
                    }
                }
                ui.add_space(10.0);
                ui.colored_label(self.ink.faint, "LINK");
                for (label, tool) in [
                    // One arrow. `(<- a b)` is `(-> b a)`, so the direction you
                    // draw already says which one you meant.
                    ("→  arrow", Tool::Flow(Form::Forward)),
                    ("┈  child of", Tool::Child),
                    ("▹  select", Tool::Select),
                ] {
                    if ui.selectable_label(self.tool == tool, label).clicked() {
                        self.tool = tool;
                        self.doc_mut().pending = None;
                    }
                }
            });
    }

    /// Keep the drawing and its source in step.
    ///
    /// Whichever side changed wins: a canvas edit regenerates the text, and
    /// text that parses replaces the drawing. Text that does not parse is left
    /// alone — you are mid-sentence, and throwing the buffer away would be the
    /// wrong response to an incomplete one.
    fn sync(&mut self) {
        let fresh = self.doc().mandala.to_rebis().ok();
        let doc = self.doc_mut();
        match fresh {
            Some(src) if src != doc.generated => {
                doc.generated = src.clone();
                doc.text = src;
            }
            _ => {}
        }
    }

    /// Adopt source typed into the panel, if it parses.
    fn adopt_text(&mut self) {
        let text = self.doc().text.clone();
        if let Ok(mandala) = Mandala::from_rebis(&text) {
            let doc = self.doc_mut();
            doc.mandala = mandala;
            doc.generated = doc.mandala.to_rebis().unwrap_or_default();
            doc.selected = None;
            doc.pending = None;
        }
    }

    fn side(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("side")
            .exact_width(330.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                if let Some(id) = self.doc_mut().selected {
                    if let Some(node) = self.doc_mut().mandala.node(id).cloned() {
                        ui.colored_label(self.ink.faint, node.form.name().to_uppercase());
                        if node.form.uses_text() {
                            let mut text = node.text.clone();
                            if ui.text_edit_singleline(&mut text).changed() {
                                self.doc_mut().mandala.set_text(id, text);
                            }
                        }
                        if let Form::Function(params) = &node.form {
                            let mut joined = params.join(" ");
                            if ui.text_edit_singleline(&mut joined).changed() {
                                let ps: Vec<String> =
                                    joined.split_whitespace().map(str::to_string).collect();
                                self.doc_mut().mandala.set_form(id, Form::Function(ps));
                            }
                            ui.colored_label(self.ink.faint, "parameters, space separated");
                        }
                        ui.colored_label(
                            self.ink.faint,
                            format!("takes {} arrows", node.form.arity()),
                        );
                        if ui.button("delete shape").clicked() {
                            self.doc_mut().mandala.remove(id);
                            self.doc_mut().selected = None;
                        }
                        ui.separator();
                    }
                }
                let k = self.ink;
                // The drawing's own diagnostic: what it cannot yet express.
                let drawing_error = self.doc().mandala.to_rebis().err().map(|e| e.to_string());
                ui.horizontal(|ui| {
                    ui.colored_label(k.faint, "REBIS");
                    let typed = self.doc().text.clone();
                    let status = if typed.trim().is_empty() {
                        String::new()
                    } else if let Some(e) = &drawing_error {
                        e.clone()
                    } else if rebis_lang::parse(&typed).is_err() {
                        "unparsed — the drawing is unchanged".to_string()
                    } else {
                        "live".to_string()
                    };
                    ui.colored_label(k.faint, status);
                });
                let mut edited = false;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let doc = self.doc_mut();
                    edited = ui
                        .add(
                            egui::TextEdit::multiline(&mut doc.text)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(24),
                        )
                        .changed();
                });
                // Typing redraws the canvas as soon as what you have typed is a
                // program.
                if edited {
                    self.adopt_text();
                }
            });
    }

    /// The run panel: the evidence a run resolves against, and the last answer.
    fn runs(&mut self, ctx: &egui::Context) {
        let k = self.ink;
        egui::TopBottomPanel::bottom("runs")
            .resizable(true)
            .default_height(150.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.colored_label(k.faint, "RECORD");
                    if ui.small_button("run this drawing").clicked() {
                        // A drawing runs by way of the source it generates, so
                        // both tabs mean the same thing by "run".
                        if let Some(Pane::Mandala(d)) = self.tabs.active() {
                            match d.mandala.to_rebis() {
                                Ok(src) => self.run_source(&src),
                                Err(e) => self.outcome = Some(e.to_string()),
                            }
                        }
                    }
                    if ui.small_button("clear").clicked() {
                        self.outcome = None;
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut self.record)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3)
                        .hint_text("one line of evidence per line"),
                );
                if let Some(out) = &self.outcome {
                    ui.separator();
                    ui.add(egui::Label::new(egui::RichText::new(out).monospace()).wrap());
                }
            });
    }

    fn footer(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(
                    self.ink.faint,
                    "drag to pan · wheel to zoom · delete removes the selection",
                );
                if let Some(id) = self.doc_mut().pending {
                    ui.colored_label(
                        self.ink.ink,
                        format!("arrow from #{} — click a target", id.0),
                    );
                }
                ui.separator();
                // The working context, shown for the same reason the terminal
                // app shows it: it decides what relative reads, imports and
                // output paths mean.
                ui.colored_label(self.ink.faint, self.cwd.display().to_string());
            });
        });
    }

    // ── canvas ─────────────────────────────────────────────────────────────

    /// Delete removes the selected shape. Ignored while a text field has
    /// focus, so editing a label is never mistaken for deleting it.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused()).is_some() {
            return;
        }
        // Tab cycling and closing go through `Tabs`, so the terminal app can
        // bind the same behaviour to its own keys without reimplementing it.
        let (next, prev, close, open) = ctx.input(|i| {
            (
                i.modifiers.ctrl && i.key_pressed(egui::Key::Tab),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowLeft),
                i.modifiers.ctrl && i.key_pressed(egui::Key::W),
                i.modifiers.ctrl && i.key_pressed(egui::Key::T),
            )
        });
        if next {
            self.tabs.next();
        }
        if prev {
            self.tabs.prev();
        }
        if open {
            let n = self.tabs.len() + 1;
            self.tabs
                .open(format!("mandala {n}"), Pane::Mandala(Doc::default()));
        }
        if close && self.tabs.len() > 1 {
            if let Some(id) = self.tabs.active_id() {
                self.tabs.close(id);
            }
        }

        let pressed = ctx.input(|i| i.key_pressed(egui::Key::Delete));
        if !pressed {
            return;
        }
        if let Some(id) = self.doc_mut().selected.take() {
            self.doc_mut().mandala.remove(id);
            self.doc_mut().pending = None;
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let origin = response.rect.min;
        // Canvas-local screen coordinates, which is the space `View` works in.
        let local = |p: Pos2| (f64::from(p.x - origin.x), f64::from(p.y - origin.y));

        // Zoom about the pointer, so the canvas grows where you are looking.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                if let Some(p) = response.hover_pos() {
                    let (sx, sy) = local(p);
                    let factor = if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 };
                    self.doc_mut().view.zoom_at(sx, sy, factor);
                }
            }
        }

        // egui separates a click from a drag for us, so there is no slop to
        // measure: `clicked()` never fires after a drag.
        if response.drag_started() {
            if let Some(p) = response.interact_pointer_pos() {
                let (sx, sy) = local(p);
                let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                self.drag = match self.doc_mut().mandala.hit(wx, wy) {
                    Some(id) => {
                        let n = self
                            .doc()
                            .mandala
                            .node(id)
                            .map(|n| (n.x, n.y))
                            .unwrap_or((wx, wy));
                        Drag::Node {
                            id,
                            grab: (wx - n.0, wy - n.1),
                        }
                    }
                    None => Drag::Pan,
                };
            }
        }
        if response.dragged() {
            match self.drag {
                Drag::Node { id, grab } => {
                    if let Some(p) = response.interact_pointer_pos() {
                        let (sx, sy) = local(p);
                        let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                        self.doc_mut().mandala.move_to(id, wx - grab.0, wy - grab.1);
                    }
                }
                Drag::Pan => {
                    let d = response.drag_delta();
                    self.doc_mut().view.pan(f64::from(d.x), f64::from(d.y));
                }
                Drag::None => {}
            }
        }
        if response.drag_stopped() {
            self.drag = Drag::None;
        }

        if response.clicked() {
            if let Some(p) = response.interact_pointer_pos() {
                let (sx, sy) = local(p);
                let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                self.click(wx, wy);
            }
        }

        // A turning ring needs a clock and a reason to redraw. egui only
        // repaints on input, so ask for frames while something is running.
        if !self.doc().running.is_empty() {
            ui.ctx().request_repaint();
        }
        self.paint(&painter, origin, ui.input(|i| i.time) as f32);
    }

    fn click(&mut self, wx: f64, wy: f64) {
        match (self.doc_mut().mandala.hit(wx, wy), self.tool.clone()) {
            // Clicked a shape.
            (Some(id), Tool::Flow(form)) => match self.doc_mut().pending {
                None => self.doc_mut().pending = Some(id),
                Some(from) => {
                    if let Some(made) = self.doc_mut().mandala.flow(from, id, form) {
                        self.doc_mut().selected = Some(made);
                    }
                    self.doc_mut().pending = None;
                }
            },
            (Some(id), Tool::Child) => match self.doc_mut().pending {
                None => self.doc_mut().pending = Some(id),
                Some(from) => {
                    self.doc_mut().mandala.connect(from, id);
                    self.doc_mut().pending = None;
                }
            },
            (Some(id), _) => self.doc_mut().selected = Some(id),
            // Clicked empty canvas.
            (None, Tool::Place(form)) => {
                let text = default_text(&form);
                let id = self.doc_mut().mandala.add(form, text, wx, wy);
                self.doc_mut().selected = Some(id);
            }
            (None, Tool::Flow(_) | Tool::Child) => self.doc_mut().pending = None,
            (None, Tool::Select) => self.doc_mut().selected = None,
        }
    }

    /// A rotating dashed ring around a node that is being evaluated.
    ///
    /// Dashes are drawn as short arcs stepped around the circle and offset by
    /// the clock, so the ring turns. It reads as motion without animating the
    /// node itself, which must stay legible while it runs.
    fn running_ring(&self, painter: &egui::Painter, centre: Pos2, zoom: f32, spin: f32) {
        const DASHES: usize = 12;
        const SEGMENTS: usize = 4;
        let r = (NODE_R as f32 + 9.0) * zoom;
        let stroke = UiStroke::new(2.2 * zoom, self.ink.accent);
        let arc = std::f32::consts::TAU / DASHES as f32;
        for dash in 0..DASHES {
            if dash % 2 == 1 {
                continue; // the gaps
            }
            let start = spin + dash as f32 * arc;
            let mut previous = None;
            for step in 0..=SEGMENTS {
                let a = start + arc * step as f32 / SEGMENTS as f32;
                let p = Pos2::new(centre.x + r * a.cos(), centre.y + r * a.sin());
                if let Some(q) = previous {
                    painter.line_segment([q, p], stroke);
                }
                previous = Some(p);
            }
        }
    }

    fn paint(&self, painter: &egui::Painter, origin: Pos2, time: f32) {
        let spin = time * 1.6;
        let k = self.ink;
        let v = self.doc().view;
        let zoom = v.zoom as f32;
        // World point to on-screen position.
        let at = |x: f64, y: f64| {
            let (sx, sy) = v.to_screen(x, y);
            Pos2::new(origin.x + sx as f32, origin.y + sy as f32)
        };

        // A flow node is drawn as the arrow between its own two children, so
        // the edges that feed it must not also be drawn — otherwise the canvas
        // shows `a -> [box] <- b` instead of `a -> b`.
        let is_flow = |id: NodeId| {
            self.doc()
                .mandala
                .node(id)
                .is_some_and(|n| n.shape() == Shape::Arrow)
        };

        // A flow node has no body, so an edge leaving one would start in empty
        // space. Start it where that flow visually ends instead — at the shape
        // its arrow points at.
        let head_of = |mut id: NodeId| {
            for _ in 0..16 {
                let Some(n) = self.doc().mandala.node(id) else {
                    break;
                };
                if n.shape() != Shape::Arrow {
                    break;
                }
                let kids = self.doc().mandala.children(id);
                let [first, second] = kids[..] else { break };
                id = if n.form == Form::Backflow {
                    first
                } else {
                    second
                };
            }
            id
        };

        // Arrows first, so shapes paint over their endpoints.
        for a in self.doc().mandala.arrows() {
            if is_flow(a.to) {
                continue;
            }
            let (Some(f), Some(t)) = (
                self.doc().mandala.node(head_of(a.from)),
                self.doc().mandala.node(a.to),
            ) else {
                continue;
            };
            let (dx, dy) = (t.x - f.x, t.y - f.y);
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            let (ux, uy) = (dx / len, dy / len);
            // Stop at the borders so the head is visible.
            let p0 = at(f.x + ux * NODE_R, f.y + uy * NODE_R);
            let p1 = at(t.x - ux * (NODE_R + 4.0), t.y - uy * (NODE_R + 4.0));
            let stroke = UiStroke::new(1.8 * zoom, k.ink);
            painter.line_segment([p0, p1], stroke);
            // Arrowhead: two short barbs swept back from the tip.
            let head = 11.0 * zoom;
            let (ax, ay) = (ux as f32, uy as f32);
            for side in [-0.45f32, 0.45] {
                let (cs, sn) = (side.cos(), side.sin());
                let (bx, by) = (ax * cs - ay * sn, ax * sn + ay * cs);
                painter.line_segment([p1, Pos2::new(p1.x - bx * head, p1.y - by * head)], stroke);
            }
        }

        // Flow nodes: one arrow between the two children, no box. `(<- a b)` is
        // `(-> b a)`, so backflow is the same line drawn the other way.
        for n in self.doc().mandala.nodes() {
            if n.shape() != Shape::Arrow {
                continue;
            }
            let kids = self.doc().mandala.children(n.id);
            let [first, second] = kids[..] else { continue };
            let (from, to) = if n.form == Form::Backflow {
                (second, first)
            } else {
                (first, second)
            };
            let (Some(f), Some(t)) = (self.doc().mandala.node(from), self.doc().mandala.node(to))
            else {
                continue;
            };
            let hot = self.doc().selected == Some(n.id) || self.doc().pending == Some(n.id);
            let (dx, dy) = (t.x - f.x, t.y - f.y);
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            let (ux, uy) = (dx / len, dy / len);
            let p0 = at(f.x + ux * NODE_R, f.y + uy * NODE_R);
            let p1 = at(t.x - ux * (NODE_R + 4.0), t.y - uy * (NODE_R + 4.0));
            let stroke = UiStroke::new(
                if hot { 2.6 } else { 1.8 } * zoom,
                if hot { k.accent } else { k.ink },
            );
            painter.line_segment([p0, p1], stroke);
            let head = 11.0 * zoom;
            let (ax, ay) = (ux as f32, uy as f32);
            for side in [-0.45f32, 0.45] {
                let (cs, sn) = (side.cos(), side.sin());
                let (bx, by) = (ax * cs - ay * sn, ax * sn + ay * cs);
                painter.line_segment([p1, Pos2::new(p1.x - bx * head, p1.y - by * head)], stroke);
            }
            // A small handle at the midpoint, so the arrow can be selected and
            // deleted like any other node. Only drawn when it is the target.
            if hot {
                painter.circle_stroke(
                    at(n.x, n.y),
                    kaos_core::visual::ARROW_HANDLE as f32 * zoom,
                    UiStroke::new(1.5 * zoom, k.accent),
                );
            }
        }

        for n in self.doc().mandala.nodes() {
            if n.shape() == Shape::Arrow {
                continue;
            }
            let hot = self.doc().selected == Some(n.id) || self.doc().pending == Some(n.id);
            let outline = UiStroke::new(
                if hot { 2.5 } else { 1.5 } * zoom,
                if hot { k.accent } else { k.faint },
            );
            let centre = at(n.x, n.y);
            let shape = n.shape();

            match shape {
                Shape::Circle => {
                    painter.circle(centre, NODE_R as f32 * zoom, k.fill, outline);
                }
                Shape::Square => {
                    painter.rect(
                        Rect::from_center_size(
                            centre,
                            Vec2::new(NODE_R as f32 * 2.0, NODE_RY as f32 * 2.0) * zoom,
                        ),
                        4.0 * zoom,
                        k.fill,
                        outline,
                    );
                }
                Shape::Diamond => {
                    let pts = Shape::diamond_points()
                        .iter()
                        .map(|(x, y)| Pos2::new(centre.x + x * zoom, centre.y + y * zoom))
                        .collect();
                    painter.add(egui::Shape::convex_polygon(pts, k.fill, outline));
                }
                // A sigil is the shape; the disc behind it is the click target
                // (see Shape::contains), drawn faintly so it reads as one mark.
                _ => {
                    painter.circle(
                        centre,
                        NODE_R as f32 * zoom,
                        k.fill,
                        UiStroke::new(1.0 * zoom, if hot { k.accent } else { k.chrome }),
                    );
                    let pen = UiStroke::new(5.0 * zoom, if hot { k.accent } else { k.ink });
                    for stroke in shape.strokes() {
                        match stroke {
                            Stroke::Poly(points) => {
                                let pts: Vec<Pos2> = points
                                    .iter()
                                    .map(|(x, y)| {
                                        Pos2::new(centre.x + x * zoom, centre.y + y * zoom)
                                    })
                                    .collect();
                                painter.add(egui::Shape::line(pts, pen));
                            }
                            Stroke::Cubic(p) => {
                                let pts = [
                                    Pos2::new(centre.x + p[0].0 * zoom, centre.y + p[0].1 * zoom),
                                    Pos2::new(centre.x + p[1].0 * zoom, centre.y + p[1].1 * zoom),
                                    Pos2::new(centre.x + p[2].0 * zoom, centre.y + p[2].1 * zoom),
                                    Pos2::new(centre.x + p[3].0 * zoom, centre.y + p[3].1 * zoom),
                                ];
                                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                                    pts,
                                    false,
                                    Color32::TRANSPARENT,
                                    pen,
                                ));
                            }
                        }
                    }
                }
            }

            if self.doc().running.contains(&n.id) {
                self.running_ring(painter, centre, zoom, spin);
            }

            let caption = n.caption();
            if caption.is_empty() {
                continue;
            }
            let caption = truncate(&caption);
            let font = FontId::monospace(11.0 * zoom);
            match shape {
                // Outlined shapes carry their label inside.
                Shape::Circle | Shape::Square | Shape::Diamond => {
                    painter.text(centre, Align2::CENTER_CENTER, caption, font, k.ink);
                }
                // A named sigil (`~ f`, `# std/flow`) keeps its name clear of
                // the glyph.
                _ => {
                    painter.text(
                        Pos2::new(centre.x, centre.y + (NODE_R as f32 + 12.0) * zoom),
                        Align2::CENTER_CENTER,
                        caption,
                        font,
                        k.faint,
                    );
                }
            }
        }
    }
}

fn default_text(form: &Form) -> String {
    Form::ALL
        .iter()
        .find(|(_, make, _)| make() == *form)
        .map(|(_, _, text)| (*text).to_string())
        .unwrap_or_default()
}

/// Keep long captions from overflowing their shape on the canvas.
fn truncate(label: &str) -> String {
    const MAX: usize = 11;
    if label.chars().count() <= MAX {
        return label.to_string();
    }
    let head: String = label.chars().take(MAX - 1).collect();
    format!("{head}…")
}

// ── terminal hand-off ───────────────────────────────────────────────────────

/// Hand the current drawing to a terminal session: write it out, then open a
/// terminal running `kaos rebis edit` on it.
///
/// The drawing leaves as ordinary Rebis source in a real file, so the terminal
/// side needs to know nothing about the canvas.
fn open_in_terminal(source: &str, cwd: &std::path::Path) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("kaos-visual-{}.rebis", std::process::id()));
    std::fs::write(&path, source)
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    let exe = std::env::current_exe().map_err(|e| format!("could not find kaos: {e}"))?;
    // The session opens in the directory the editor was started from, so
    // relative reads, imports and output paths mean there what they mean here.
    launch_terminal(
        &format!(
            "{} rebis edit {}",
            shell_quote(&exe.to_string_lossy()),
            shell_quote(&path.to_string_lossy())
        ),
        cwd,
    )
}

/// Single-quote a path for `sh -c`, so spaces and metacharacters survive.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(target_os = "macos")]
fn launch_terminal(command: &str, cwd: &std::path::Path) -> Result<(), String> {
    // AppleScript is the only reliable way to get a *new* Terminal window
    // running a command; `open -a Terminal` cannot take arguments. `cd` first
    // so the session starts in the same working context.
    let full = format!("cd {} && {command}", shell_quote(&cwd.to_string_lossy()));
    let escaped = full.replace('\\', r"\\").replace('"', r#"\""#);
    let script = format!(
        r#"tell application "Terminal"
             activate
             do script "{escaped}"
           end tell"#
    );
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open Terminal: {e}"))
}

#[cfg(not(target_os = "macos"))]
fn launch_terminal(command: &str, cwd: &std::path::Path) -> Result<(), String> {
    // There is no standard terminal on Linux/BSD, so try the usual suspects in
    // order. `x-terminal-emulator` is the Debian alternatives entry and so
    // respects the user's own choice when it exists.
    const TERMINALS: &[(&str, &[&str])] = &[
        ("x-terminal-emulator", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("konsole", &["-e"]),
        ("xfce4-terminal", &["-e"]),
        ("alacritty", &["-e"]),
        ("kitty", &["-e"]),
        ("wezterm", &["start", "--"]),
        ("foot", &["-e"]),
        ("xterm", &["-e"]),
    ];
    // Keep the shell open afterwards so the session is usable, not a flash.
    let inner = format!("{command}; exec \"$SHELL\"");
    for (bin, args) in TERMINALS {
        let spawned = std::process::Command::new(bin)
            .args(*args)
            .arg("sh")
            .arg("-c")
            .arg(&inner)
            // The session inherits the editor's working context.
            .current_dir(cwd)
            .spawn();
        if spawned.is_ok() {
            return Ok(());
        }
    }
    Err("no terminal found (tried gnome-terminal, konsole, alacritty, kitty, xterm…)".into())
}
