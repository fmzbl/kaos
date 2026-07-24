//! kaos — the terminal app for the Pact.
//!
//! A red-themed REPL whose commands are `/slash`-prefixed. Type a bare intent to
//! set the adept to work; use `/cast` for a one-shot rite; use `/help` for the
//! grimoire. One-shot mode mirrors every command (`kaos bench 300 2026`,
//! `kaos cast "fix the deadlock"`, `kaos roster`, …).

use std::io::{self, Write};

use kaos::agent::{self, AdeptAgent, ModelAgent, ModelBackend, ScriptedAgent};
use kaos::backend::adept_system_prompt;
use kaos::conductor::{Chat, Conductor, ProviderChat, Step, Tool};
use kaos::gnosis::{assemble, Current};
use kaos::order::Pact;
use kaos::provider::{Kind, Spec};
use kaos::ray::Ray;
use kaos::rebis_workspace::HypersigilModules;
use kaos::rite::{perform, Levers, Rite};
use kaos::rng::{hash_str, Rng};
use kaos::sigil::{statement_of_intent, Sigil};
use kaos::theme::*;

/// Session state carried across turns: the Pact (whose grades and egregore evolve),
/// the summoned mind, and the running seed.
struct Session {
    pact: Pact,
    /// The mind the live rites (`cast`, `code`) summon.
    model: Spec,
    rng: Rng,
    /// Files attached to the conversation (path, contents) — prepended, verbatim
    /// (never compressed), to what the mind sees on `cast`/`conclave`/`myth`.
    attachments: Vec<(String, String)>,
}

impl Session {
    fn new() -> Session {
        // The TUI (and any caller) pins the mind via KAOS_MODEL so one-shot
        // subprocesses inherit the same summoning.
        let model = std::env::var("KAOS_MODEL")
            .ok()
            .map(|s| Spec::parse(&s))
            .unwrap_or_else(Spec::simulated);
        Session {
            pact: Pact::convene(),
            model,
            rng: Rng::new(seed_from_clock()),
            attachments: Vec::new(),
        }
    }
}

/// The attached files as a verbatim context block (empty if none). Prepended to
/// the mind's input so reference material rides along uncompressed.
fn attachment_block(session: &Session) -> String {
    if session.attachments.is_empty() {
        return String::new();
    }
    let mut s = String::from("Attached files for context:\n");
    for (path, content) in &session.attachments {
        s.push_str(&format!("\n===== {path} =====\n{content}\n"));
    }
    s.push_str("\n===== end of attachments =====\n\n");
    s
}

/// /attach <path> — read a file into the conversation. `/attach` lists them;
/// `/attach clear` drops them.
fn attach_cmd(session: &mut Session, arg: &str) {
    let arg = arg.trim();
    if arg.is_empty() || arg == "list" {
        if session.attachments.is_empty() {
            println!("  {}", ash("no files attached. /attach <path> to add one."));
        } else {
            println!("  {}", bold(RED(), "ATTACHED"));
            for (p, c) in &session.attachments {
                println!(
                    "    {} {}",
                    fg((190, 150, 90), p),
                    dim(ASH(), &format!("({} bytes)", c.len()))
                );
            }
        }
        return;
    }
    if arg == "clear" {
        let n = session.attachments.len();
        session.attachments.clear();
        println!("  {}", ash(&format!("detached {n} file(s).")));
        return;
    }
    match std::fs::read_to_string(arg) {
        Ok(c) => {
            session.attachments.retain(|(p, _)| p != arg);
            println!(
                "  {} {} {}",
                fg((90, 200, 110), "\u{2734} attached"),
                bone(arg),
                dim(ASH(), &format!("({} bytes)", c.len()))
            );
            session.attachments.push((arg.to_string(), c));
        }
        Err(e) => println!(
            "  {} {}",
            fg(RED(), &format!("could not read {arg}:")),
            ash(&e.to_string())
        ),
    }
}

/// Drag-and-drop: terminals paste a dropped file as its path — often quoted, or
/// with backslash-escaped spaces, or as a `file://` URI. If a whole line is
/// nothing but existing file path(s), return them (unescaped) so they can be
/// attached; empty otherwise, so a normal intent is never mistaken for a drop.
fn dropped_paths(line: &str) -> Vec<String> {
    let toks = split_drop(line);
    if toks.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for t in &toks {
        let p = unwrap_path(t);
        if std::path::Path::new(&p).is_file() {
            out.push(p);
        } else {
            return Vec::new(); // any non-file token → this is an intent, not a drop
        }
    }
    out
}

fn unwrap_path(t: &str) -> String {
    let t = t.strip_prefix("file://").unwrap_or(t);
    let t = t.trim_matches(|c| c == '\'' || c == '"');
    t.replace("\\ ", " ").replace("\\\\", "\\")
}

/// Split a line into tokens on unquoted whitespace, honouring quotes and
/// backslash-escapes (how terminals emit dropped paths).
fn split_drop(line: &str) -> Vec<String> {
    let (mut toks, mut cur) = (Vec::new(), String::new());
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                cur.push('\\');
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                }
            }
            '\'' | '"' => {
                if quote == Some(c) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(c);
                }
                cur.push(c);
            }
            c if c.is_whitespace() && quote.is_none() => {
                if !cur.is_empty() {
                    toks.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    toks
}

fn main() {
    if let Err(error) = kaos::config::load() {
        eprintln!("config: {error}");
    }
    kaos::auth::load(); // seed the env from ~/.config/kaos/credentials (never over an export)
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        // Interactive with no args → the fullscreen ratatui app (when built with the
        // default `tui` feature and attached to a real terminal). Otherwise the plain
        // line REPL, which also serves pipes/CI.
        #[cfg(feature = "tui")]
        {
            use std::io::IsTerminal;
            if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
                if let Err(e) = kaos::tui::run() {
                    eprintln!("tui error: {e}");
                }
                return;
            }
        }
        repl();
        return;
    }
    // One-shot mode.
    let mut session = Session::new();
    let cmd = args[0].as_str();
    let rest = if cmd == "code" && kaos::config::enabled("KAOS_RAW_CHAT_TASK_STDIN") {
        let mut task = String::new();
        if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut task) {
            eprintln!("could not read chat task from stdin: {error}");
            return;
        }
        task
    } else {
        args[1..].join(" ")
    };
    match cmd {
        "repl" | "chat" => repl(),
        "cast" | "summon" => cast(&mut session, &rest),
        "attach" | "file" => attach_cmd(&mut session, &rest),
        "auth" | "login" => auth_cmd(&rest),
        "scry" => scry(&rest),
        "roster" => print_roster(&session.pact),
        "conclave" => conclave(&session, &rest),
        "rebis" => rebis_cmd(&session, &rest),
        "visual" => visual_cmd(&rest),
        "edit" => rebis_screen(&rest),
        // Kept as an undocumented compatibility command for existing workflows.
        "myth" => myth_screen(&session),
        "mirror" => rebis_cmd(&session, &rest),
        "egregore" => print_egregore(&session.pact),
        "forge" | "solve" => forge_cmd(&rest),
        "code" | "conduct" => code_cmd(&session, &rest),
        "models" | "minds" => models_cmd(&session, &rest),
        "help" | "-h" | "--help" => print_help(),
        other => {
            // Treat any other leading word as the start of a task to cast.
            let task = std::iter::once(other.to_string())
                .chain(args[1..].iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            cast(&mut session, &task);
        }
    }
}

// ───────────────────────────── the REPL ─────────────────────────────

fn repl() {
    let mut session = Session::new();
    banner(&session);
    // Plain line input (std-only). The fullscreen TUI is the interactive front door;
    // this REPL serves pipes, CI, and --no-default-features builds.
    let mut input = kaos::input::Prompt::new();
    loop {
        match input.read(&prompt()) {
            kaos::input::Line::Eof => {
                println!("\n{}", ash("the temple closes. \u{2734}"));
                break;
            }
            kaos::input::Line::Text(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if !dispatch(&mut session, line) {
                    break;
                }
            }
        }
    }
}

/// Run one REPL line. Returns false to quit.
fn dispatch(session: &mut Session, line: &str) -> bool {
    if let Some(rest) = line.strip_prefix('/') {
        let mut it = rest.splitn(2, char::is_whitespace);
        let cmd = it.next().unwrap_or("");
        let arg = it.next().unwrap_or("").trim();
        match cmd {
            "help" | "?" => print_help(),
            "cast" | "summon" => cast(session, arg),
            "attach" | "file" => attach_cmd(session, arg),
            "auth" | "login" => auth_cmd(arg),
            "scry" => scry(arg),
            "roster" | "pact" => print_roster(&session.pact),
            "conclave" => conclave(session, arg),
            "rebis" => rebis_screen(arg),
            // Legacy evaluator compatibility; Rebis is the user-facing language.
            "myth" => myth_screen(session),
            "egregore" | "mind" => print_egregore(&session.pact),
            "forge" | "solve" => forge_cmd(arg),
            "code" | "conduct" => code_cmd(session, arg),
            "models" | "minds" => models_cmd(session, arg),
            "model" | "backend" | "bind" => model_cmd(session, arg),
            "banish" => banish_session(session),
            "rays" | "magics" => print_rays(),
            "quit" | "exit" | "q" => return false,
            "" => println!(
                "{}",
                ash("speak a command after the slash; /help lists them.")
            ),
            other => println!(
                "{}",
                ash(&format!(
                    "no rite named /{other}. /help lists the grimoire."
                ))
            ),
        }
    } else {
        // Drag-and-drop: a line that is only file path(s) attaches them.
        let dropped = dropped_paths(line);
        if !dropped.is_empty() {
            for p in dropped {
                attach_cmd(session, &p);
            }
        } else {
            // A bare line is an INTENT for the agent — it works the current
            // directory with real tools, and is literal chat text, never
            // `/code` grammar. `/cast` remains for the one-shot.
            code_task(session, line, true);
        }
    }
    true
}

// ──────────────────────────── commands ─────────────────────────────

/// /cast — perform the Great Work on a task. Simulated by default; with the claude
/// backend, fires the charged sigil at a real adept.
fn cast(session: &mut Session, task: &str) {
    if task.is_empty() {
        println!(
            "{}",
            ash("name the intent. \u{2734} /cast fix the deadlock in the worker pool")
        );
        return;
    }
    if session.model.kind == Kind::Simulated {
        let rite = perform(&mut session.pact, task, Levers::full(), &mut session.rng);
        render_rite(&rite);
    } else {
        cast_live(session, task);
    }
}

/// The live rite: sigilize, route to an adept, and fire the charged intent at the
/// summoned mind. The verbose statement of intent is *banished* — only the
/// compressed charge is sent.
fn cast_live(session: &mut Session, task: &str) {
    let ray = Ray::classify(task);
    let sigil = Sigil::construct(&statement_of_intent(task));
    let idx = session.pact.route(ray);
    let adept = &session.pact.members[idx];

    println!();
    println!("{}", rule(60));
    println!(
        "  {} {}  {}",
        bold(RED(), "RITE"),
        bone(task),
        dim(ASH(), &format!("[{}]", session.model.label())),
    );
    render_sigil_block(&sigil, ray);
    println!(
        "  {} {} {} {}",
        ash("bound to"),
        bold(ray.rgb(), &adept.name),
        dim(ASH(), adept.grade.degree()),
        dim(ASH(), &format!("({} ray)", ray.name())),
    );
    println!(
        "  {} {}",
        ash("charge \u{2192}"),
        bone(&sigil.charged_intent)
    );
    println!("{}", rule(60));

    let system = adept_system_prompt(&adept.name, ray.name(), ray.sphere());
    let user = format!("{}{}", attachment_block(session), sigil.charged_intent);
    match session.model.complete(
        &system,
        &user,
        std::time::Duration::from_secs(call_timeout_s()),
    ) {
        Ok(reply) => {
            println!("{}", reply);
            // No gate, no Weighing, no elevation: a reply that merely *arrived* was
            // not weighed true — there is no verifier here to judge the work. Grades
            // and the egregore move only where a verdict exists (the simulated rite,
            // whose equation decides, or a gated conclave). Anything else would let
            // transport success masquerade as competence.
        }
        Err(e) => {
            println!(
                "{}",
                fg(RED(), &format!("\u{2734} the charge fizzles \u{2014} {e}"))
            );
        }
    }
}

/// /scry — dry-run a task: classify the ray, construct the sigil, and show the
/// equation that *would* be assembled — without charging. Pure inspection.
fn scry(task: &str) {
    if task.is_empty() {
        println!("{}", ash("scry what? /scry optimize the query planner"));
        return;
    }
    let ray = Ray::classify(task);
    let sigil = Sigil::construct(&statement_of_intent(task));

    // Assemble the equation against the fittest adept, with no egregore, fresh R.
    let pact = Pact::convene();
    let idx = pact.route(ray);
    let mut rng = Rng::new(sigil.signature);
    let (eq, current) = assemble(
        &pact.members[idx],
        sigil.awareness(),
        ray,
        0.0,
        0.10,
        &mut rng,
    );

    println!();
    println!("{}", rule(60));
    println!("  {} {}", bold(RED(), "SCRY"), bone(task));
    render_sigil_block(&sigil, ray);
    println!(
        "  {} {} {}",
        ash("fittest adept"),
        bold(ray.rgb(), &pact.members[idx].name),
        dim(
            ASH(),
            &format!(
                "{} {}",
                pact.members[idx].grade.degree(),
                pact.members[idx].grade.title()
            )
        ),
    );
    render_equation(&eq, current);
    let m = eq.magic_factor();
    println!(
        "  {} {}",
        ash("forecast M (cold, no shared mind):"),
        bold(RED(), &format!("{:.1}%", m * 100.0)),
    );

    // The divination proper: the SECOND equation, Pm = P + (1−P)·M^(1/P), read as
    // a spending guide. P is the chance the work lands by one ungated charge; the
    // mid-band law says where a conclave pays and where it is waste. A scry cannot
    // know P — it names the band, honestly, and leaves the estimate to the operator.
    println!();
    println!(
        "  {}",
        ash("the second equation \u{2014} what this M buys at each base chance P:")
    );
    for p in [0.1_f64, 0.3, 0.5, 0.7, 0.9] {
        let pm = kaos::equation::probability_shift(p, m);
        let gain = kaos::equation::lift(p, m);
        let bar = "\u{2588}".repeat((gain * 40.0).round() as usize);
        println!(
            "    {}  {} {}  {}",
            dim(ASH(), &format!("P={p:.1}")),
            ash(&format!("\u{2192} Pm={pm:.2}")),
            fg(RED(), &bar),
            dim(ASH(), &format!("(+{gain:.2})")),
        );
    }
    println!(
        "  {}",
        dim(
            ASH(),
            "the lift peaks mid-band. reading: if one shot usually lands, cast alone;"
        )
    );
    println!("  {}", dim(ASH(), "if it sometimes lands, convene a gated conclave (/code xK \u{2026} -- <tests>) \u{2014}"));
    println!(
        "  {}",
        dim(
            ASH(),
            "the quorum adjourns when settled; if it never lands, no k rescues it: reframe."
        )
    );
    println!(
        "{}",
        dim(
            ASH(),
            "  (no charge fired — scrying only. /cast to perform the work.)"
        )
    );
    println!("{}", rule(60));
}

/// `/auth` — provider credentials, stored 0600 in ~/.config/kaos/credentials and
/// loaded at startup. No args: status. `<provider> <key>`: store. `forget <p>`:
/// clear. `claude`: the subscription CLI needs a login, not a key.
fn auth_cmd(arg: &str) {
    let arg = arg.trim();
    let mut it = arg.splitn(2, char::is_whitespace);
    let first = it.next().unwrap_or("").trim();
    let second = it.next().unwrap_or("").trim();

    // The claude CLI authenticates through the claude.ai subscription, not a key.
    if matches!(
        first.to_lowercase().as_str(),
        "claude" | "claude-cli" | "cli"
    ) {
        println!();
        println!("  {}", bold(RED(), "auth \u{2014} claude CLI"));
        println!(
            "  {}",
            ash("the claude backend uses your claude.ai login, not an API key.")
        );
        println!("  {} {}", dim(ASH(), "log in:"), bone("claude login"));
        println!(
            "  {}",
            ash("kaos strips ANTHROPIC_API_KEY on this path so a stray key can't hijack it.")
        );
        return;
    }

    if first.eq_ignore_ascii_case("forget") || first.eq_ignore_ascii_case("logout") {
        match kaos::auth::forget(second) {
            Ok(var) => println!(
                "  {} {}",
                fg(RED(), "\u{2734} forgot"),
                ash(&format!("{var} cleared from store and env"))
            ),
            Err(e) => println!("  {} {}", fg(RED(), "\u{2734}"), ash(&e.to_string())),
        }
        return;
    }

    // `<provider> <key>` → store it (persisted + live this session).
    if !first.is_empty() && !second.is_empty() {
        match kaos::auth::store(first, second) {
            Ok((var, path)) => {
                println!(
                    "  {} {}",
                    bold(RED(), "\u{25c9} stored"),
                    ash(&format!("{var} saved to {}", path.display()))
                );
                println!(
                    "  {}",
                    dim(
                        ASH(),
                        "it is live now and loads automatically next session."
                    )
                );
            }
            Err(e) => println!("  {} {}", fg(RED(), "\u{2734}"), ash(&e.to_string())),
        }
        return;
    }

    // `<provider>` alone → show how to set it.
    if !first.is_empty() {
        match kaos::auth::var_for(first) {
            Some(var) => println!(
                "  {} {}",
                ash(&format!("give {first} a key:")),
                bone(&format!("/auth {first} <{var}>"))
            ),
            None => println!(
                "  {} {}",
                fg(RED(), "\u{2734}"),
                ash(&format!(
                    "unknown provider '{first}' \u{2014} openrouter | openai | anthropic | claude"
                ))
            ),
        }
        return;
    }

    // No args → the status board.
    println!();
    println!("  {}", bold(RED(), "AUTH \u{2014} provider credentials"));
    println!("{}", rule(64));
    for (name, var, live, saved) in kaos::auth::status() {
        let mark = if live {
            fg((90, 200, 120), "\u{25cf} set  ")
        } else {
            dim(ASH(), "\u{25cb} unset")
        };
        let src = if saved {
            "(stored)"
        } else if live {
            "(env)"
        } else {
            ""
        };
        println!(
            "    {name:<11} {}  {}  {}",
            mark,
            dim(ASH(), var),
            dim(ASH(), src)
        );
    }
    println!(
        "    {:<11} {}  {}",
        "claude",
        fg((90, 200, 120), "\u{25cf} login"),
        dim(
            ASH(),
            "claude.ai subscription \u{2014} `claude login`, no key"
        )
    );
    println!("{}", rule(64));
    println!(
        "  {} {}",
        dim(ASH(), "set:   "),
        bone("/auth openrouter sk-or-...")
    );
    println!(
        "  {} {}",
        dim(ASH(), "clear: "),
        bone("/auth forget openrouter")
    );
}

/// /conclave <task> — run the default myth (a voted best-of-k) on the bound
/// mind. Override the myth with `KAOS_MYTH="(gather vote (spread 8 fire))"`.
fn conclave(session: &Session, task: &str) {
    if task.is_empty() {
        println!(
            "{}",
            ash("conclave for what? /conclave what is 12! mod 1000")
        );
        return;
    }
    let ray = Ray::classify(task);
    println!();
    println!(
        "  {} {} {}",
        bold(RED(), "CONCLAVE"),
        ash("for the"),
        bold(ray.rgb(), &format!("{} ray", ray.name())),
    );
    if session.model.kind == Kind::Simulated {
        println!("  {}", ash("bind a live mind with /model to run."));
        return;
    }
    let k = std::env::var("KAOS_K")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5usize)
        .max(1);
    let sexpr =
        std::env::var("KAOS_MYTH").unwrap_or_else(|_| format!("(gather vote (spread {k} fire))"));
    run_myth(session, &sexpr, task);
}

/// Parse a myth S-expression and run it on the bound mind, printing the myth and
/// the collapsed verdict. Shared by `/conclave` and `/myth`.
fn run_myth(session: &Session, sexpr: &str, task: &str) {
    let node = match kaos::myth::parse(sexpr) {
        Ok(n) => n,
        Err(e) => {
            println!("  {} {}", fg(RED(), "\u{2734} myth:"), ash(&e));
            return;
        }
    };
    if let Err(e) = session.model.readiness() {
        println!(
            "  {} {}",
            fg(RED(), "\u{2734} the mind is unreachable \u{2014}"),
            ash(&e)
        );
        return;
    }
    println!("{}", rule(64));
    println!("  {}  {}", bold(RED(), "myth"), dim(ASH(), sexpr));
    let task = format!("{}{}", attachment_block(session), task);
    // KAOS_AGENTIC turns every leaf into a real Conductor session (read/edit/bash in
    // an isolated copy) instead of a single completion — the myth *acts*. Its verdict
    // is the winning patchset; without it, leaves are plain answers.
    let timeout = std::time::Duration::from_secs(call_timeout_s());
    let verdict = if kaos::config::enabled("KAOS_AGENTIC") {
        let root = std::env::var("KAOS_ARENA").unwrap_or_else(|_| ".".to_string());
        let steps = max_steps();
        let leaves = node.leaves();
        let conc = std::env::var("KAOS_MAX_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3usize)
            .max(1);
        println!(
            "  {}  {}",
            dim(ASH(), "agentic"),
            dim(ASH(), &format!("leaves act in copies of {root}"))
        );
        // Cost exposure up front — an agentic leaf is a whole session, not one call.
        println!(
            "  {}  {}",
            fg((210, 160, 60), "\u{26a0} cost"),
            ash(&format!(
                "up to {leaves} sessions \u{00d7} {steps} steps = ~{} model calls on {} ({} at a time). ^C to abort.",
                leaves * steps,
                session.model.label(),
                conc,
            )),
        );
        let cast = kaos::solve::AgentCast {
            spec: &session.model,
            timeout,
            root: root.into(),
            max_steps: max_steps(),
            bash_timeout_s: std::env::var("KAOS_BASH_TIMEOUT_S")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(600),
            gate_timeout_s: std::env::var("KAOS_GATE_TIMEOUT_S")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
        };
        kaos::myth::run(&node, &task, &cast)
    } else {
        let cast = kaos::solve::ChatCast {
            spec: &session.model,
            timeout,
        };
        kaos::myth::run(&node, &task, &cast)
    };
    match verdict {
        Some(a) => println!(
            "  {} {}",
            bold(RED(), "\u{25c9} verdict"),
            bone(&kaos::solve::render_verdict(&a))
        ),
        None => println!(
            "  {}",
            fg(RED(), "\u{2734} no answer \u{2014} every branch fizzled")
        ),
    }
}

/// Open KAOS's Rebis model-interface workspace. In a terminal this is the native
/// Vim-like editor and live graph view; through a pipe it emits the compiled graph
/// as Graphviz so scripts can consume the same lowering.
fn rebis_screen(arg: &str) {
    #[cfg(feature = "tui")]
    {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            let path = (!arg.trim().is_empty()).then_some(arg.trim());
            if let Err(error) = kaos::tui::run_rebis(path) {
                eprintln!("rebis workspace: {error}");
            }
            return;
        }

        if arg.trim().is_empty() {
            println!("{}", kaos::theme::chaos_star_red());
            return;
        }
        let source = if std::path::Path::new(arg.trim()).is_file() {
            match std::fs::read_to_string(arg.trim()) {
                Ok(source) => source,
                Err(error) => {
                    eprintln!("rebis: could not read {}: {error}", arg.trim());
                    return;
                }
            }
        } else {
            arg.trim().to_string()
        };
        match rebis_lang::parse(&source) {
            Ok(expr) => print!("{}", rebis_lang::tree(&expr)),
            Err(error) => eprintln!("rebis: {error}"),
        }
    }
    #[cfg(not(feature = "tui"))]
    {
        let _ = arg;
        eprintln!("rebis workspace requires the default `tui` feature");
    }
}

/// `/myth` — legacy compatibility for the original composition evaluator.
/// Rebis is the user-facing model-interface language and workspace.
fn myth_screen(session: &Session) {
    print!("\x1b[2J\x1b[H"); // clear + home
    let _ = io::stdout().flush();
    println!();
    println!("  {}", bold(RED(), "THE MYTH \u{2014} weave a myth"));
    println!("{}", rule(64));
    println!(
        "  {}",
        dim(
            ASH(),
            "a myth is an S-expression graph — a layer over kaos:"
        )
    );
    println!(
        "    {}  {}",
        bold((190, 150, 90), "fire         "),
        ash("one model call")
    );
    println!(
        "    {}  {}",
        bold((190, 150, 90), "(ask \"role\")  "),
        ash("a call with an instruction   (a stage's job)")
    );
    println!(
        "    {}  {}",
        bold((190, 150, 90), "(spread N X) "),
        ash("run X, N ways   (diverge)")
    );
    println!(
        "    {}  {}",
        bold((190, 150, 90), "(gather G X) "),
        ash("collapse X via gate G   (converge)")
    );
    println!(
        "    {}  {}",
        bold((190, 150, 90), "(pipe A B …) "),
        ash("each stage's answer feeds the next   (sequence)")
    );
    println!(
        "    {}  {}",
        dim(ASH(), "G ="),
        dim(ASH(), "vote  |  first  |  (check \"shell-cmd\")")
    );
    println!("  {}", dim(ASH(), "e.g.  (gather vote (spread 5 fire))"));
    println!(
        "  {}",
        dim(
            ASH(),
            "      (pipe (ask \"Propose a fix\") (ask \"Critique it\") (ask \"Write final code\"))"
        )
    );
    println!("{}", rule(64));
    if session.model.kind == Kind::Simulated {
        println!("  {}", ash("bind a live mind with /model to run a myth."));
        return;
    }
    let mut input = kaos::input::Prompt::new();
    let sexpr = loop {
        match input.read(&format!("  {} ", fg(RED(), "myth \u{25b8}"))) {
            kaos::input::Line::Eof => return,
            kaos::input::Line::Text(t) if t.trim().is_empty() => return,
            kaos::input::Line::Text(t) => match kaos::myth::parse(t.trim()) {
                Ok(_) => break t.trim().to_string(),
                Err(e) => println!("  {} {}", fg(RED(), "\u{2734}"), ash(&e)),
            },
        }
    };
    match input.read(&format!("  {} ", fg(RED(), "task \u{25b8}"))) {
        kaos::input::Line::Text(t) if !t.trim().is_empty() => run_myth(session, &sexpr, t.trim()),
        _ => {}
    }
}

/// /roster — the full Pact, by grade.
fn print_roster(pact: &Pact) {
    println!();
    println!("  {}", bold(RED(), "THE PACT"));
    println!("{}", rule(60));
    for (n, line) in pact.roster().iter().enumerate() {
        let colour = if n == 0 { RED() } else { ASH() };
        println!("  {}", fg(colour, line));
    }
    println!("{}", rule(60));
    println!(
        "  {}",
        dim(
            ASH(),
            &format!(
                "egregore awakeness: {:.0}%",
                pact.egregore.awakeness() * 100.0
            )
        ),
    );
}

/// /egregore — the shared mind: its awakeness and its recent distilled lessons.
fn print_egregore(pact: &Pact) {
    println!();
    println!(
        "  {}",
        bold(RED(), "THE EGREGORE \u{2014} the Pact's shared mind")
    );
    println!(
        "  {} {}",
        ash("awakeness"),
        bold(RED(), &format!("{:.0}%", pact.egregore.awakeness() * 100.0)),
    );
    if pact.egregore.ledger.is_empty() {
        println!(
            "  {}",
            dim(
                ASH(),
                "(no lessons yet — cast some rites to feed the mind.)"
            )
        );
    } else {
        for line in pact.egregore.ledger.iter().rev().take(12) {
            println!("    {}", ash(line));
        }
    }
}

/// /code — a REAL tool-using agentic loop. The adept reads, edits, and runs commands
/// in a target directory until the task is done.
///
/// Usage: `code [dir] [xK] <task…> [-- <verify cmd> | -- none]`
///   - `dir` defaults to the current directory.
///   - With no `xK`, the app sizes the working ITSELF: the project's own test
///     command is divined (tests.py, pytest, cargo, npm, make — override with
///     `-- <cmd>`, opt out with `-- none`) and the ADAPTIVE quorum runs — one
///     attempt first, growing (max 4) only while the Weighing keeps failing,
///     each retry carrying the gate's verdict. No gate → a single adept.
///   - `xK` (e.g. `x5`) overrides: a fixed conclave of K adepts in isolated
///     copies, verified best-of-k, consensus diff ships.
///
/// Per-call HTTP timeout for API minds (`KAOS_TIMEOUT_S`, default 120). Repo-scale
/// work can push one completion (a whole-file write) past two minutes.
fn call_timeout_s() -> u64 {
    std::env::var("KAOS_TIMEOUT_S")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120)
}

/// A bare chat message starts a complete tool-using coding session. Keep its
/// model turns independently configurable from short, one-shot completions;
/// older configs without the dedicated key retain their former 180s floor.
fn chat_timeout_s() -> u64 {
    std::env::var("KAOS_CHAT_TIMEOUT_S")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or_else(|| call_timeout_s().max(180))
}

/// Rebis model calls have their own timeout. Normal mode makes one direct call
/// per prompt; chaos mode may use the whole allowance for a multi-step agent.
fn rebis_timeout_s() -> u64 {
    std::env::var("KAOS_REBIS_TIMEOUT_S")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or_else(|| chat_timeout_s().max(600))
}

/// Paradigm-switching restarts (belief-as-tool, from PsyberMagick): the spiral's
/// banished retries adopt DIFFERENT solving approaches instead of re-sampling the
/// same one. The measured failure of a weak model is fixating on a single wrong
/// hypothesis across restarts; rotating the paradigm breaks that. `""` = the plain
/// first attempt. Disabled with `KAOS_NO_PARADIGM=1`.
fn paradigm(i: usize) -> &'static str {
    if std::env::var("KAOS_NO_PARADIGM")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return "";
    }
    match i {
        0 => "",
        1 => {
            "Reproduce the failure with a minimal check first, then fix the root cause it \
              depends on — not the first thing that merely looks wrong."
        }
        2 => {
            "Assume your previous approach was mistaken. Re-read the relevant code from \
              scratch and consider a DIFFERENT cause for the failure."
        }
        _ => {
            "Question every assumption. Trace the data end to end; the defect likely sits \
              where you have not yet looked. State each fix as a concrete change you verify."
        }
    }
}

/// The Open Hand switch (`KAOS_HAND=1`): drive HTTP minds through their NATIVE
/// tool-calling dialect instead of the parsed <act> protocol. Same tools, same
/// wards, same ladders — the k2.7 finding made the dialect itself the variable.
#[cfg(feature = "api")]
fn open_hand(spec: &Spec) -> bool {
    std::env::var("KAOS_HAND")
        .map(|v| v == "1")
        .unwrap_or(false)
        && matches!(spec.kind, Kind::OpenRouter | Kind::OpenAi | Kind::Ollama)
}

/// The Dream (G12, `src/dream.rs`): a toolless, lunar-temperature divination
/// over a banished working, returning a seed hypothesis for the next self —
/// or "" when dreaming is off (`KAOS_NO_DREAM=1`), unavailable, or empty.
/// The dream touches nothing; only the gate ever ships work.
fn dream_between(spec: &Spec, task: &str, gnosis: &str) -> String {
    if std::env::var("KAOS_NO_DREAM")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return String::new();
    }
    if gnosis.trim().is_empty() {
        return String::new();
    }
    let (system, user) = kaos::dream::dream_prompt(task, gnosis);
    let mut sampling = kaos::backend::Sampling::seeded(hash_str(&format!("dream|{task}")));
    sampling.temperature = kaos::spiral::Polarity::Lunar.temperature(); // the divinatory pole
    let reply = spec.complete_sampled(
        &system,
        &user,
        std::time::Duration::from_secs(call_timeout_s().min(90)),
        Some(sampling),
    );
    match reply.ok().and_then(|r| kaos::dream::distill(&r)) {
        Some(h) => kaos::dream::seed(&h),
        None => String::new(),
    }
}

/// Run one bounded session, using the native dialect when that feature is available.
/// the given sampling. The seam every spiral/forge/audit path dispatches through.
struct SessionObservers<'a> {
    on_model_call: &'a mut dyn FnMut(usize),
    on_model_reply: &'a mut dyn FnMut(usize, &str),
    on_step: &'a mut dyn FnMut(&Step),
}

fn run_session(
    root: &std::path::Path,
    intent: &str,
    spec: &Spec,
    sampling: kaos::backend::Sampling,
    max_steps_n: usize,
    on_step: &mut dyn FnMut(&Step),
) -> kaos::conductor::Session {
    let mut on_model_call = |_: usize| {};
    let mut on_model_reply = |_: usize, _: &str| {};
    run_session_with_timeout(
        root,
        intent,
        spec,
        sampling,
        max_steps_n,
        chat_timeout_s(),
        None,
        SessionObservers {
            on_model_call: &mut on_model_call,
            on_model_reply: &mut on_model_reply,
            on_step,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn run_session_with_timeout(
    root: &std::path::Path,
    intent: &str,
    spec: &Spec,
    sampling: kaos::backend::Sampling,
    max_steps_n: usize,
    timeout_s: u64,
    system_appendix: Option<String>,
    observers: SessionObservers<'_>,
) -> kaos::conductor::Session {
    let mut conductor = Conductor::new(root);
    conductor.max_steps = max_steps_n;
    conductor.system_appendix = system_appendix;
    #[cfg(feature = "api")]
    if open_hand(spec) {
        return conductor.run_native_observed(
            intent,
            spec,
            Some(sampling),
            std::time::Duration::from_secs(timeout_s),
            observers.on_model_call,
            observers.on_model_reply,
            observers.on_step,
        );
    }
    let chat = ProviderChat {
        spec: spec.clone(),
        timeout_s,
        sampling: Some(sampling),
    };
    conductor.run_observed(
        intent,
        &chat,
        observers.on_model_call,
        observers.on_model_reply,
        observers.on_step,
    )
}

/// Step budget for the <act> loop (`KAOS_MAX_STEPS`, default 14). 14 suits the
/// small devbench arenas; a large repo needs room to explore before editing.
fn max_steps() -> usize {
    std::env::var("KAOS_MAX_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(14)
}

/// `kaos mirror "<expr>"` — evaluate a mirror-language expression. The record
/// is read from stdin when piped (`cat notes.md | kaos mirror '(-> "a" "b")'`); with
/// no stdin the record is empty and scores use the term-coverage fallback.
/// The language is atoms, grouping, `->` (collide) and `<-` (backflow); see
/// docs/REBIS.md.
struct RebisOracle<'a> {
    model: &'a Spec,
    root: &'a std::path::Path,
    allow_tools: bool,
    chaos: bool,
    /// In a hosted run this is a renewable slice: reaching it pauses before
    /// the next model call, and SIGCONT grants another slice.
    model_call_slice: usize,
    /// Interpreter prompt position, including answers replayed from a previous
    /// child after an interruption.
    sequence: std::cell::Cell<usize>,
    /// New model calls made by this child. Replayed calls do not consume a
    /// renewed model-call slice or trigger its pause boundary again.
    live_sequence: std::cell::Cell<usize>,
    journal: kaos::rebis_checkpoint::PromptJournal,
    /// Read before each unfinished prompt. The TUI may update this file while
    /// the interpreter is alive; checkpoint replays intentionally bypass it.
    directive_path: Option<std::path::PathBuf>,
}

impl RebisOracle<'_> {
    fn pause_failed_prompt(&self, error: &str) -> bool {
        kaos::pause::enabled() && kaos::pause::current_run(error)
    }

    fn finish_prompt(
        &self,
        index: usize,
        prompt: &str,
        answer: Option<String>,
    ) -> Result<Option<String>, String> {
        loop {
            match self.journal.record(index, prompt, answer.as_deref()) {
                Ok(()) => return Ok(answer),
                Err(error) if self.pause_failed_prompt(&error) => {}
                Err(error) => return Err(error),
            }
        }
    }
}

/// The input seam for `(& port body)` in a hosted run.
///
/// When the program reaches a port with no value yet, the child parks itself
/// through the same cooperative pause protocol used for transient model errors,
/// but with an `awaiting input` reason so the TUI knows to wait for the user
/// rather than auto-resume. On `SIGCONT` it reads the value the TUI delivered
/// and continues. Outside a cooperative TUI (no delivery file) the port simply
/// has no value.
struct RebisInlet {
    path: Option<std::path::PathBuf>,
}

impl rebis_lang::Inlet for RebisInlet {
    fn receive(&self, port: &str) -> Option<String> {
        let path = self.path.as_deref()?;
        // A value delivered before we blocked (or pre-seeded) is taken directly.
        if let Some(value) = kaos::rebis_inlet::take_input(path, port) {
            return Some(value);
        }
        // Otherwise stop until the host delivers a value and resumes us. A
        // resume with nothing delivered (a spurious SIGCONT) parks again.
        loop {
            if !kaos::pause::current_run(&kaos::rebis_inlet::await_reason(port)) {
                // Pause is not enabled: we cannot block, so the port is unbound.
                return None;
            }
            if let Some(value) = kaos::rebis_inlet::take_input(path, port) {
                return Some(value);
            }
        }
    }
}

impl rebis_lang::Oracle for RebisOracle<'_> {
    fn fire(&self, prompt: &str) -> Option<String> {
        self.try_fire(prompt).ok().flatten()
    }

    fn try_fire(&self, prompt: &str) -> Result<Option<String>, String> {
        let mut sampling = kaos::backend::Sampling::seeded(hash_str(&format!(
            "rebis-agent|{}|{prompt}",
            if self.chaos { "chaos" } else { "normal" }
        )));
        sampling.temperature = kaos::spiral::Polarity::Solar.temperature();
        let agent = self.sequence.get() + 1;
        self.sequence.set(agent);
        if let kaos::rebis_checkpoint::Replay::Hit(answer) = self.journal.replay(agent - 1, prompt)
        {
            println!("checkpoint replayed · Rebis prompt {agent} already complete");
            let _ = std::io::stdout().flush();
            return Ok(answer);
        }
        let directive = self
            .directive_path
            .as_deref()
            .and_then(kaos::rebis_supervisor::read_directive);
        let effective_prompt =
            kaos::rebis_supervisor::directed_prompt(prompt, directive.as_deref());
        if directive.is_some() {
            println!("supervisor directive active · Rebis run guidance attached");
            let _ = std::io::stdout().flush();
        }
        let mut step = 0usize;
        let live_agent = self.live_sequence.get() + 1;
        self.live_sequence.set(live_agent);
        if self.model_call_slice > 0
            && live_agent > 1
            && (live_agent - 1).checked_rem(self.model_call_slice) == Some(0)
        {
            let _ = kaos::pause::current_run(&format!(
                "model call limit ({}) reached without a failure",
                self.model_call_slice
            ));
        }
        kaos::fold::open(&format!(
            "Rebis agent {agent} · {}",
            effective_prompt.lines().next().unwrap_or("working")
        ));
        let timeout_s = rebis_timeout_s();
        let model_label = self.model.label();
        if !self.allow_tools {
            println!("model    generating turn 1 · {model_label} · NORMAL · limit {timeout_s}s");
            let _ = std::io::stdout().flush();
            let system = kaos::conductor::rebis_agent_system_prompt();
            let response = loop {
                match self.model.complete_sampled(
                    &system,
                    &effective_prompt,
                    std::time::Duration::from_secs(timeout_s),
                    Some(sampling),
                ) {
                    Ok(response) if response.trim().is_empty() => {
                        if kaos::pause::current_run("model returned no answer") {
                            continue;
                        }
                        break response;
                    }
                    Ok(response) => break response,
                    Err(error) if self.pause_failed_prompt(&error) => continue,
                    Err(error) => {
                        println!("model    failed · {error}");
                        kaos::fold::close();
                        return Err(error);
                    }
                }
            };
            kaos::fold::open("model turn 1 · complete response");
            if response.is_empty() {
                println!("model    (empty response)");
            } else {
                for line in response.lines() {
                    println!("model    {line}");
                }
            }
            kaos::fold::close();
            let _ = std::io::stdout().flush();
            let answer = response.trim().to_string();
            kaos::fold::close();
            return self.finish_prompt(agent - 1, prompt, (!answer.is_empty()).then_some(answer));
        }

        let root = std::fs::canonicalize(self.root).unwrap_or_else(|_| self.root.to_path_buf());
        if !self.chaos && self.model.kind == Kind::ClaudeCli {
            println!("model    direct Claude agent · {model_label} · native tools enabled");
            let task = format!(
                "{}\n\nYou are operating directly in this workspace:\n{}\n\nUse your native \
                 file and command tools to perform every requested change. Do not merely describe \
                 edits. Complete only this Rebis node, then return the value that should flow to \
                 the next node.\n\nNODE PROMPT:\n{effective_prompt}",
                kaos::conductor::rebis_agent_system_prompt(),
                root.display()
            );
            let response = loop {
                let response = kaos::backend::run_claude_agent_once_with_result(
                    &root,
                    &task,
                    self.model.claude_tag(),
                    |line| {
                        for rendered in kaos::backend::claude_event_lines(line) {
                            println!("{rendered}");
                        }
                        let _ = std::io::stdout().flush();
                    },
                );
                match response {
                    Ok(response) if response.trim().is_empty() => {
                        if kaos::pause::current_run("model returned no answer") {
                            continue;
                        }
                        break response;
                    }
                    Ok(response) => break response,
                    Err(error) if self.pause_failed_prompt(&error) => continue,
                    Err(error) => {
                        println!("model    failed · {error}");
                        kaos::fold::close();
                        return Err(error);
                    }
                }
            };
            let answer = response.trim().to_string();
            kaos::fold::close();
            return self.finish_prompt(agent - 1, prompt, (!answer.is_empty()).then_some(answer));
        }

        println!(
            "model    {} · {model_label} · tools enabled · limit {timeout_s}s",
            if self.chaos {
                "Kaos chaos pipeline"
            } else {
                "direct node tool agent"
            }
        );
        let task = format!(
            "You are a Rebis agent operating in this workspace:\n{}\n\n\
             Use the provided tools to inspect the workspace and to perform every file edit or \
             command requested by the instruction. Do not merely describe changes that the \
             instruction asks you to make. Complete the work in this launch directory, verify it \
             when appropriate, and finish with the value that should flow to the next Rebis \
             node:\n\n{effective_prompt}",
            root.display()
        );
        let mut on_model_call = |turn: usize| {
            println!("model    generating turn {turn} · {model_label} · limit {timeout_s}s");
            let _ = std::io::stdout().flush();
        };
        let mut on_model_reply = |turn: usize, response: &str| {
            kaos::fold::open(&format!("model turn {turn} · complete response"));
            if response.is_empty() {
                println!("model    (empty response)");
            } else {
                for line in response.lines() {
                    println!("model    {line}");
                }
            }
            kaos::fold::close();
            let _ = std::io::stdout().flush();
        };
        // One bounded Conductor run per node is the only tool mechanism an
        // HTTP backend offers; the appended contract scopes it to one direct
        // node agent, while --chaos keeps the unscoped Kaos pipeline.
        let session = loop {
            let session = run_session_with_timeout(
                &root,
                &task,
                self.model,
                sampling,
                max_steps(),
                timeout_s,
                (!self.chaos).then(kaos::conductor::rebis_node_tool_contract),
                SessionObservers {
                    on_model_call: &mut on_model_call,
                    on_model_reply: &mut on_model_reply,
                    on_step: &mut |event| {
                        step += 1;
                        render_step(step, event);
                    },
                },
            );
            let Some(error) = session.error.as_deref() else {
                if session.final_message.trim().is_empty()
                    && self.pause_failed_prompt("model returned no answer")
                {
                    continue;
                }
                break session;
            };
            let error = if error.to_ascii_lowercase().contains("timed out") {
                format!(
                    "{error}; this Rebis model turn is limited by \
                     KAOS_REBIS_TIMEOUT_S={timeout_s} (raise it for slower models)"
                )
            } else {
                error.to_string()
            };
            println!("model    failed · {error}");
            if !self.pause_failed_prompt(&error) {
                kaos::fold::close();
                return Err(error);
            }
        };
        let answer = session.final_message.trim().to_string();
        if answer.is_empty() {
            println!("model    nothing");
        } else {
            for line in answer.lines() {
                println!("model    {line}");
            }
        }
        kaos::fold::close();
        self.finish_prompt(agent - 1, prompt, (!answer.is_empty()).then_some(answer))
    }
}

struct DryOracle;

impl rebis_lang::Oracle for DryOracle {
    fn fire(&self, _prompt: &str) -> Option<String> {
        None
    }
}

/// `kaos visual [program-or-file]` — the mandala editor. Draw `o-[]-o`, get
/// Rebis source; or load an existing program onto the canvas.
fn visual_cmd(arg: &str) {
    #[cfg(feature = "visual")]
    {
        // A second front door onto the standalone editor — `kaos-visual` runs
        // the identical code without this app installed at all.
        match kaos::visual_ui::open(arg) {
            Ok(mandala) => kaos::visual_ui::run(mandala),
            Err(error) => {
                eprintln!("visual: {error}");
                std::process::exit(2);
            }
        }
    }
    #[cfg(not(feature = "visual"))]
    {
        // Without the window, still report whether the program is drawable.
        let arg = arg.trim();
        if !arg.is_empty() {
            let source = std::fs::read_to_string(arg).unwrap_or_else(|_| arg.to_string());
            match kaos::visual::Mandala::from_rebis(&source) {
                Ok(m) => println!("visual: drawable — {} shapes", m.nodes().len()),
                Err(e) => {
                    eprintln!("visual: {e}");
                    std::process::exit(2);
                }
            }
        }
        eprintln!(
            "kaos visual needs the `visual` feature:\n  \
             cargo install --path . --features visual\n\
             Or run the editor on its own:\n  \
             cargo install --path kaos-visual && kaos-visual"
        );
        std::process::exit(2);
    }
}

fn rebis_cmd(session: &Session, arg: &str) {
    let mut parts = arg.trim().splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    let (visualization, expr_src) = if first == "edit" {
        rebis_screen(parts.next().unwrap_or_default().trim());
        return;
    } else if first == "tree" || first == "mandala" {
        (Some(first), parts.next().unwrap_or_default().trim())
    } else if first == "run" {
        let run_arg = parts.next().unwrap_or_default().trim();
        rebis_run_cmd(session, run_arg);
        return;
    } else {
        (None, arg.trim())
    };
    if expr_src.is_empty() {
        eprintln!("usage: kaos rebis edit [file] | tree|mandala|run <program-or-file>");
        std::process::exit(2);
    }
    let mut texts: Vec<String> = Vec::new();
    {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf);
            if !buf.trim().is_empty() {
                texts.push(buf);
            }
        }
    }
    let record = rebis_lang::Record::from_texts(&texts);
    if let Some(visualization) = visualization {
        match rebis_lang::parse(expr_src) {
            Ok(expr) => {
                if visualization == "mandala" {
                    print!("{}", rebis_lang::mandala(&expr));
                } else {
                    println!("{}", rebis_lang::REBIS_SIGIL);
                    if record.is_empty() {
                        print!("{}", rebis_lang::tree(&expr));
                    } else {
                        print!("{}", rebis_lang::tree_scored(&expr, &record));
                    }
                }
            }
            Err(error) => eprintln!("rebis: {error}"),
        }
        return;
    }
    match rebis_lang::run(expr_src, &record) {
        Ok(concept) => {
            println!("score    {:.3}", concept.score);
            println!(
                "terms    {}",
                concept.terms.iter().cloned().collect::<Vec<_>>().join(" ")
            );
            println!("evidence {} line(s)", concept.evidence.len());
            if record.is_empty() {
                println!("note     empty record: scores use the term-coverage fallback");
            }
        }
        Err(e) => {
            eprintln!("rebis: {e}");
            std::process::exit(2);
        }
    }
}

fn rebis_run_cmd(session: &Session, arg: &str) {
    let mut dry = false;
    let mut allow_tools = false;
    let mut chaos = false;
    let mut source_arg = arg.trim();
    loop {
        let mut parts = source_arg.splitn(2, char::is_whitespace);
        let flag = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default().trim();
        match flag {
            "--dry" => dry = true,
            "--allow-tools" => allow_tools = true,
            "--chaos" => chaos = true,
            _ => break,
        }
        source_arg = rest;
    }
    if source_arg.is_empty() {
        eprintln!("usage: kaos rebis run [--dry] [--allow-tools] [--chaos] <program-or-file>");
        std::process::exit(2);
    }
    if !dry && chaos && !allow_tools {
        eprintln!(
            "rebis: chaos mode can edit files and run commands; approve with `--allow-tools`"
        );
        std::process::exit(4);
    }
    if !dry {
        // This child owns one Rebis run, so its explicit authority applies to
        // every direct or chaos agent beneath it without duplicate prompts.
        std::env::set_var("KAOS_CLAUDE_YOLO", if allow_tools { "1" } else { "0" });
    }
    let source = if std::path::Path::new(source_arg).is_file() {
        match std::fs::read_to_string(source_arg) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("rebis: could not read {source_arg}: {error}");
                std::process::exit(2);
            }
        }
    } else {
        source_arg.to_string()
    };
    let expr = match rebis_lang::parse(&source) {
        Ok(expr) => expr,
        Err(error) => {
            eprintln!("rebis: {error}");
            std::process::exit(2);
        }
    };
    let mut input = String::new();
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
    }
    let mut record = rebis_lang::Record::from_texts(&[input]);
    if !dry {
        if session.model.kind == Kind::Simulated {
            eprintln!("rebis: bind a live mind with KAOS_MODEL, or use `run --dry`");
            std::process::exit(2);
        }
        if let Err(error) = session.model.readiness() {
            eprintln!("rebis: model unavailable: {error}");
            std::process::exit(2);
        }
    }
    let mut stream = |event: &rebis_lang::ExecutionEvent| {
        use rebis_lang::{ExecutionEvent, FlowDirection};
        match event {
            ExecutionEvent::PromptStarted {
                prompt,
                abstraction,
            } => {
                let head = prompt.lines().next().unwrap_or_default();
                println!("event    prompt started · abstraction {abstraction} · {head}");
            }
            ExecutionEvent::PromptFinished(firing) => {
                for line in firing.answer.as_deref().unwrap_or("nothing").lines() {
                    println!("answer   {line}");
                }
            }
            ExecutionEvent::FlowRouted { direction, .. } => println!(
                "event    {} value routed",
                match direction {
                    FlowDirection::Forward => "forward",
                    FlowDirection::Backflow => "backflow",
                }
            ),
            ExecutionEvent::MediatorStarted { branches } => {
                println!("event    mediator started · {branches} branch(es)");
            }
            ExecutionEvent::BranchSelected { decision } => println!(
                "event    conditional selected {} branch",
                if *decision { "yes" } else { "no" }
            ),
            ExecutionEvent::MediatorResolved { result, holonomy } => println!(
                "event    mediator resolved deterministically · result {result} · holonomy {holonomy}%"
            ),
            ExecutionEvent::MacroExpanded { name, remaining } => {
                println!("event    macro {name} expanded · {remaining} remaining");
            }
            ExecutionEvent::SyntaxInverted => println!("event    syntax orientation inverted"),
            ExecutionEvent::InputReceived { port, value } => {
                let head = value.lines().next().unwrap_or_default();
                println!("event    input received on {port} · {head}");
            }
            ExecutionEvent::ModuleLoaded {
                module,
                definitions,
            } => println!("event    module {module} loaded · {definitions} definition(s)"),
            ExecutionEvent::Diagnostic(diagnostic) => println!("diagnostic {diagnostic}"),
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());
    };
    let modules = HypersigilModules::user(
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    );
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let limit = |name: &str, default| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let model_call_slice = limit("KAOS_REBIS_MAX_CALLS", rebis_lang::MAX_MODEL_CALLS);
    let limits = rebis_lang::RuntimeLimits::standard()
        .with_macro_expansions(limit(
            "KAOS_REBIS_MAX_EXPANSIONS",
            rebis_lang::MAX_MACRO_EXPANSIONS,
        ))
        .with_module_imports(limit(
            "KAOS_REBIS_MAX_MODULES",
            rebis_lang::MAX_MODULE_IMPORTS,
        ))
        // Hosted runs renew this allowance cooperatively inside RebisOracle, so
        // the language engine must not turn the boundary into a diagnostic.
        .with_model_calls(if kaos::pause::enabled() {
            usize::MAX
        } else {
            model_call_slice
        })
        .with_max_concurrency(limit(
            "KAOS_REBIS_MAX_CONCURRENCY",
            rebis_lang::MAX_CONCURRENCY,
        ));
    // Dry evaluation is safe to fan out. Tool-using agents share one real
    // workspace, so execute them sequentially in program order: concurrent file
    // edits would violate Rebis's isolation assumptions and race each other.
    let result = if dry {
        rebis_lang::orchestrate_parallel(
            &expr,
            &mut record,
            &DryOracle,
            &modules,
            limits,
            &mut stream,
        )
    } else {
        rebis_lang::orchestrate_with_inlet(
            &expr,
            &mut record,
            &RebisOracle {
                model: &session.model,
                root: &root,
                allow_tools,
                chaos,
                model_call_slice,
                sequence: std::cell::Cell::new(0),
                live_sequence: std::cell::Cell::new(0),
                journal: kaos::rebis_checkpoint::PromptJournal::from_env(),
                directive_path: kaos::rebis_supervisor::path_from_env(),
            },
            &modules,
            &RebisInlet {
                path: kaos::rebis_inlet::path_from_env(),
            },
            limits,
            &mut stream,
        )
    };
    println!("{}", rebis_lang::REBIS_SIGIL);
    println!("RESULT");
    if let Some(output) = &result.output {
        for line in output.lines() {
            println!("result   {line}");
        }
    } else {
        println!("result   nothing");
    }
    println!("TRACE");
    for (index, firing) in result.firings.iter().enumerate() {
        println!("firing   {} · {}", index + 1, firing.agent);
        println!("answer   {}", firing.answer.as_deref().unwrap_or("nothing"));
    }
    println!("score    {:.3}", result.concept.score);
    println!(
        "terms    {}",
        result
            .concept
            .terms
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!("evidence {} line(s)", result.concept.evidence.len());
    if !result.diagnostics.is_empty() {
        println!("DIAGNOSTICS");
        for diagnostic in &result.diagnostics {
            println!("diagnostic {diagnostic}");
        }
        let _ = std::io::stdout().flush();
        std::process::exit(3);
    }
}

fn code_cmd(session: &Session, arg: &str) {
    code_task(
        session,
        arg,
        kaos::config::enabled("KAOS_RAW_CHAT_TASK_STDIN"),
    )
}

/// `raw_task` marks `arg` as one literal chat intent for the current directory,
/// bypassing the `/code` grammar (`[dir] [xK] task -- gate`) so text containing
/// ` -- `, an `x3` token, or a directory-looking word is never rewritten or
/// truncated on its way to the agent.
fn code_task(session: &Session, arg: &str, raw_chat_task: bool) {
    if arg.trim().is_empty() {
        println!(
            "{}",
            ash("code what? e.g. /code . add a --version flag to the CLI")
        );
        println!(
            "{}",
            dim(
                ASH(),
                "  conclave: /code x5 fix the parser -- python3 check.py"
            )
        );
        return;
    }

    // A literal task arriving from the chat composer is not `/code` syntax.
    // In particular, pasted source may legitimately contain ` -- `, start with
    // `x3`, or name a directory; none of those may rewrite or truncate it.
    let (head, verify_cmd) = if raw_chat_task {
        (arg, None)
    } else {
        // Split off an optional `-- <verify cmd>` gate before tokenising the rest.
        match arg.split_once(" -- ") {
            Some((h, v)) if !v.trim().is_empty() => (h.trim(), Some(v.trim().to_string())),
            _ => (arg.trim(), None),
        }
    };

    // Peel leading [dir] and [xK] tokens off the head; the remainder is the task.
    let mut root = ".".to_string();
    let mut k: Option<usize> = None;
    let mut rest = head;
    if !raw_chat_task {
        loop {
            let mut it = rest.splitn(2, char::is_whitespace);
            let first = it.next().unwrap_or("");
            let after = it.next().unwrap_or("").trim();
            if let Some(n) = first
                .strip_prefix('x')
                .and_then(|d| d.parse::<usize>().ok())
            {
                k = Some(n.max(1));
                rest = after;
            } else if std::path::Path::new(first).is_dir() && !after.is_empty() {
                root = first.to_string();
                rest = after;
            } else {
                break;
            }
        }
    }
    let task = rest.trim().to_string();
    if task.is_empty() {
        println!("{}", ash("give the adept a task: /code <dir> <what to do>"));
        return;
    }

    // Gate divination: when the user gives none, the project is read for its own
    // Weighing (tests.py, pytest, cargo, npm, make) — `-- none` opts out. The app
    // sizes the working itself; no one has to know what a conclave is.
    let root_probe = std::path::PathBuf::from(&root);
    let (verify_cmd, divined) = match verify_cmd.as_deref() {
        Some("none") | Some("nogate") => (None, false),
        Some(g) => (Some(g.to_string()), false),
        None => match kaos::conductor::detect_gate(&root_probe) {
            Some(g) => (Some(g), true),
            None => (None, false),
        },
    };

    // Sizing: an explicit xK is honoured exactly (K>1 = the isolated-copy
    // conclave). Otherwise the ADAPTIVE quorum decides for itself: one attempt
    // first, growing only while the gate keeps failing (max 4). No gate → a
    // single adept; sampling more without a Weighing is spend without a selector.
    let adaptive = k.is_none() && verify_cmd.is_some();
    let k = k.unwrap_or(1);

    // The agent needs a live mind. A simulated session has none — default to the
    // claude CLI so `/code` works out of the box.
    let spec = if session.model.kind == Kind::Simulated {
        Spec::new(Kind::ClaudeCli, "claude")
    } else {
        session.model.clone()
    };
    if let Err(e) = spec.readiness() {
        println!(
            "  {} {}",
            fg(RED(), "\u{2734} the mind is unreachable \u{2014}"),
            ash(&e)
        );
        return;
    }

    let root_path = std::path::PathBuf::from(&root);
    println!();
    println!("  {}  {}", bold(RED(), "CONDUCT"), bone(&task));
    println!(
        "  {} {}   {} {}",
        ash("ground"),
        dim(
            ASH(),
            &std::fs::canonicalize(&root)
                .map(|p| p.display().to_string())
                .unwrap_or(root.clone())
        ),
        ash("mind"),
        dim(ASH(), &spec.label()),
    );
    if k > 1 {
        let gate = verify_cmd.as_deref().unwrap_or("(none — consensus only)");
        println!(
            "  {} {}   {} {}",
            ash("conclave"),
            bold(RED(), &format!("k={k} adepts, verified best-of-k")),
            ash("gate"),
            dim(ASH(), gate),
        );
    } else if adaptive {
        println!(
            "  {} {}   {} {}",
            ash("quorum"),
            bold(RED(), "adaptive — grows only if the Weighing fails"),
            ash(if divined { "gate (divined)" } else { "gate" }),
            dim(ASH(), verify_cmd.as_deref().unwrap_or("")),
        );
    }
    println!(
        "  {}",
        dim(
            OXBLOOD(),
            "the adept reads, edits, runs \u{2014} until the Work is done"
        )
    );

    // The inner working, foldable: exactly what is being summoned, with nothing
    // hidden — the mind, the protocol, the budgets, the seeds, the gate. In the
    // TUI this is a collapsed group; the plain CLI skips it (compactness rules).
    if kaos::fold::enabled() {
        kaos::fold::open(&dim(
            ASH(),
            "  \u{2699} the inner working \u{2014} what is really happening",
        ));
        println!("  {}", ash(&format!("mind: {}", spec.label())));
        if spec.kind == Kind::ClaudeCli && k == 1 {
            println!(
                "  {}",
                dim(
                    ASH(),
                    "protocol: the whole task is delegated to the claude CLI as its own agent"
                )
            );
            println!("  {}", dim(ASH(), "          (claude brings its own read/edit/bash tools; kaos streams its output)"));
            let sid = std::env::var("KAOS_SESSION").unwrap_or_default();
            let resumed = std::env::var("KAOS_RESUME")
                .map(|v| v == "1")
                .unwrap_or(false);
            if !sid.is_empty() {
                println!(
                    "  {}",
                    dim(
                        ASH(),
                        &format!(
                            "memory: claude conversation {sid} ({})",
                            if resumed {
                                "resumed — it remembers prior turns"
                            } else {
                                "created fresh this turn"
                            }
                        )
                    )
                );
            }
        } else {
            println!("  {}", dim(ASH(), "protocol: kaos's own <act> loop \u{2014} each turn the model must answer with ONE"));
            println!("  {}", dim(ASH(), "          tool call (read_file/write_file/edit_file/bash/finish); kaos executes"));
            println!("  {}", dim(ASH(), "          it and appends the observation to the transcript; repeat until finish"));
            println!(
                "  {}",
                dim(ASH(), "budgets: 14 steps max \u{b7} 120s per bash command")
            );
        }
        if k > 1 {
            println!("  {}", dim(ASH(), &format!("conclave: {k} adepts, each in an ISOLATED COPY of the target (their messes never mix)")));
            println!(
                "  {}",
                dim(
                    ASH(),
                    &format!(
                        "weighing: {}",
                        verify_cmd
                            .as_deref()
                            .map(|g| format!(
                                "`{g}` runs in each copy; only gate-passing diffs may ship"
                            ))
                            .unwrap_or_else(|| {
                                "NO GATE \u{2014} consensus-only, an honestly weaker signal".into()
                            })
                    )
                )
            );
            println!(
                "  {}",
                dim(
                    ASH(),
                    "vote: the modal verified change-set ships; the quorum ADJOURNS early once the"
                )
            );
            println!(
                "  {}",
                dim(
                    ASH(),
                    "      leader cannot be overtaken \u{2014} remaining adepts are never summoned"
                )
            );
            println!("  {}", dim(ASH(), &format!("sampling: each adept gets a distinct seed derived from hash(task|i) at temp 0.7{}", if spec.kind == Kind::Ollama { "" } else { " (honoured on ollama minds)" })));
        } else if adaptive {
            println!("  {}", dim(ASH(), "quorum: ADAPTIVE \u{2014} attempt 1 runs alone; only a failed Weighing summons another"));
            println!("  {}", dim(ASH(), "        adept (max 4), each with a fresh context carrying the gate's verdict as a"));
            println!("  {}", dim(ASH(), "        distilled memory (retroactive enchantment) and a distinct sampling seed"));
            println!(
                "  {}",
                dim(
                    ASH(),
                    &format!(
                        "weighing: `{}`{}",
                        verify_cmd.as_deref().unwrap_or(""),
                        if divined {
                            " \u{2014} DIVINED from the project's own files (override: -- <cmd>, or -- none)"
                        } else {
                            ""
                        }
                    )
                )
            );
            println!("  {}", dim(ASH(), "in place: edits land in the real tree (review with git diff), like a single adept"));
        }
        let yolo = std::env::var("KAOS_CLAUDE_YOLO")
            .map(|v| v == "1")
            .unwrap_or(false);
        println!(
            "  {}",
            dim(
                ASH(),
                &format!(
                    "authority: {}",
                    if yolo {
                        "unbound \u{2014} the adept may run shell"
                    } else {
                        "edits only (claude path) \u{2014} note: the <act> loop's bash tool is not gated by this"
                    }
                )
            )
        );
        kaos::fold::close();
    }
    println!("{}", rule(64));

    if k > 1 {
        code_conclave(&root_path, &task, k, verify_cmd.as_deref(), &spec);
        return;
    }

    // ── the adaptive quorum (the default with a gate, on non-claude minds) ──
    // The claude CLI is its own agent with its own retry judgment; adaptation is
    // for the raw-completion minds kaos conducts itself.
    if adaptive && spec.kind != Kind::ClaudeCli {
        let gate = verify_cmd.as_deref().unwrap_or("");
        let spec_for = spec.clone();
        let task_for = task.clone();
        // Shared fold state between the two closures (steps stream INTO a fold per
        // attempt; verdict lines print at top level) — Cells, since both borrow it.
        let fold_open = std::cell::Cell::new(false);
        let step_n = std::cell::Cell::new(0usize);
        let cur_attempt = std::cell::Cell::new(usize::MAX);
        let root_for = root_path.clone();
        let outcome = kaos::conductor::run_adaptive_with(
            &root_path,
            &task,
            gate,
            move |i, intent, steps_budget, on| {
                // Each turn of the spiral samples from the other universe:
                // solar (cold) first, its lunar twin (hot) on the banished retry.
                let mut sampling =
                    kaos::backend::Sampling::seeded(hash_str(&format!("adaptive|{task_for}|{i}")));
                sampling.temperature = kaos::spiral::Polarity::of_attempt(i).temperature();
                // Paradigm-switching restart (belief-as-tool): the banished retry
                // adopts a DIFFERENT approach, not just a reseed — a weak model's
                // measured failure is fixating on one wrong hypothesis across turns.
                let framed = match paradigm(i) {
                    "" => intent.to_string(),
                    p => format!("{intent}\n\nApproach for this attempt: {p}"),
                };
                run_session(&root_for, &framed, &spec_for, sampling, steps_budget, on)
            },
            {
                let spec_d = spec.clone();
                let task_d = task.clone();
                move |gnosis: &str| dream_between(&spec_d, &task_d, gnosis)
            },
            &kaos::spiral::budgets(max_steps().max(16) * 3),
            120,
            |attempt, step| {
                // Each attempt's steps live in their own fold, opened lazily.
                if attempt != cur_attempt.get() {
                    if fold_open.get() {
                        kaos::fold::close();
                    }
                    kaos::fold::open(&format!(
                        "\u{25c9} attempt {}  \u{2014} the adept works\u{2026}",
                        attempt + 1
                    ));
                    fold_open.set(true);
                    cur_attempt.set(attempt);
                    step_n.set(0);
                }
                step_n.set(step_n.get() + 1);
                render_step(step_n.get(), step);
            },
            |line| {
                if fold_open.get() {
                    kaos::fold::close();
                    fold_open.set(false);
                }
                println!("  {}", ash(line));
            },
        );
        println!("{}", rule(64));
        if outcome.verified {
            println!(
                "  {}  {}",
                bold(
                    (90, 200, 110),
                    "\u{2734} the Work is done \u{2014} weighed true"
                ),
                dim(
                    ASH(),
                    &format!("({} attempt(s); review with git diff)", outcome.attempts)
                ),
            );
        } else {
            println!(
                "  {}  {}",
                fg(RED(), "\u{2734} the Weighing never passed"),
                ash(&format!(
                    "after {} attempt(s) \u{2014} the edits stand for your review; the gate says what remains",
                    outcome.attempts
                )),
            );
        }
        return;
    }

    // ── single adept (k=1) ──
    // The `claude` CLI is its OWN agent (own read/edit/bash tools), so delegate the
    // whole task to it in the target dir. Other minds are raw completion endpoints
    // with no tools — drive them through our own <act> conductor loop.
    if spec.kind == Kind::ClaudeCli {
        match kaos::backend::run_claude_agent(&root_path, &task, spec.claude_tag(), |line| {
            for rendered in kaos::backend::claude_event_lines(line) {
                println!("{rendered}");
            }
        }) {
            Ok(()) => println!("\n  {}", bold((90, 200, 110), "\u{2734} the Work is done")),
            Err(e) => println!(
                "\n  {} {}",
                fg(RED(), "\u{2734} the Work falters \u{2014}"),
                ash(&e)
            ),
        }
        return;
    }

    // ── the Forge ──
    // No gate was given or divined — but the Weighing is the whole edge, so
    // FORGE one: a fib-budgeted phase-zero session writes kaos_repro.py, a
    // standalone script distilled from the issue that exits non-zero while
    // the bug lives and 0 once it is fixed. If the forged sigil verifiably
    // FAILS right now, it becomes the gate and the full machinery engages —
    // spiral, gnosis crossing, lunar audit. If the forge fizzles, fall
    // through to the gateless spiral unchanged.
    let forge_enabled = !std::env::var("KAOS_NO_FORGE")
        .map(|v| v == "1")
        .unwrap_or(false);
    if forge_enabled {
        let forge_gate = "python3 kaos_repro.py";
        println!("  {}", ash("\u{2692} the Forge \u{2014} no gate exists, so one is being forged from the issue\u{2026}"));
        let mut sampling = kaos::backend::Sampling::seeded(hash_str(&format!("forge|{task}")));
        sampling.temperature = kaos::spiral::Polarity::Solar.temperature();
        let forge_task = format!(
            "{task}\n\nDo NOT fix anything yet. Your ONLY job: write kaos_repro.py in the \
             project root — a standalone script that REPRODUCES the reported bug: it must \
             exit non-zero (assert or sys.exit(1)) while the bug exists and exit 0 once the \
             bug is fixed. Locate the relevant code fast (grep, narrow the candidates each \
             round), write the script, run `{forge_gate}` to CONFIRM it currently fails, \
             then finish."
        );
        let mut n_forge = 0;
        let forge_session = run_session(&root_path, &forge_task, &spec, sampling, 8, &mut |step| {
            n_forge += 1;
            render_step(n_forge, step);
        });
        // The forged sigil must exist and must FAIL now — a repro that already
        // passes proves nothing and would gate the quorum on a lie.
        let forged = root_path.join("kaos_repro.py").exists() && {
            let out = kaos::conductor::run_shell(&root_path, forge_gate, 120);
            !out.starts_with("exit 0")
        };
        if forged {
            println!("  {}", bold(RED(), "\u{2692} the gate is forged \u{2014} kaos_repro.py fails as the issue describes"));
            let gnosis = kaos::spiral::gnosis(&forge_session);
            // The Lost Sigil (G14): the metric is banished from the context.
            // The self is told a hidden Weighing judges it — never which file,
            // never its contents; the gate runs automatically after each
            // attempt and the executor seals the verifier against mutation.
            let intent = format!(
                "{task}\n\nA hidden Weighing, forged from this issue, judges every attempt \
                 automatically — you cannot see or alter it. Reproduce the reported behaviour \
                 yourself, fix the SOURCE code only (never anything under a tests/ directory), \
                 verify by exercising the behaviour, then finish. What the forging \
                 established:\n{gnosis}"
            );
            let spec_for = spec.clone();
            let task_for = task.clone();
            let root_for = root_path.clone();
            let outcome = kaos::conductor::run_adaptive_with(
                &root_path,
                &intent,
                forge_gate,
                move |i, attempt_intent, steps_budget, on| {
                    let mut s = kaos::backend::Sampling::seeded(hash_str(&format!(
                        "forged|{task_for}|{i}"
                    )));
                    s.temperature = kaos::spiral::Polarity::of_attempt(i).temperature();
                    run_session(&root_for, attempt_intent, &spec_for, s, steps_budget, on)
                },
                {
                    let spec_d = spec.clone();
                    let task_d = task.clone();
                    move |gnosis: &str| dream_between(&spec_d, &task_d, gnosis)
                },
                &kaos::spiral::budgets(max_steps().max(16) * 3),
                120,
                |_, step| {
                    n_forge += 1;
                    render_step(n_forge, step);
                },
                |line| println!("  {}", ash(line)),
            );
            let _ = std::fs::remove_file(root_path.join("kaos_repro.py"));
            println!("{}", rule(64));
            if outcome.verified {
                println!(
                    "  {}  {}",
                    bold(
                        (90, 200, 110),
                        "\u{2734} the Work is done \u{2014} weighed true by the forged gate"
                    ),
                    dim(ASH(), &format!("({} attempt(s))", outcome.attempts)),
                );
            } else {
                println!(
                    "  {}  {}",
                    fg(RED(), "\u{2734} the forged Weighing never passed"),
                    ash(&format!(
                        "after {} attempt(s) \u{2014} the edits stand for review",
                        outcome.attempts
                    )),
                );
            }
            return;
        }
        println!("  {}", ash("\u{2692} the forge fizzled \u{2014} no verifiable repro; the gateless spiral proceeds"));
        let _ = std::fs::remove_file(root_path.join("kaos_repro.py"));
    }

    // ── the gateless spiral ──
    // No gate to weigh by, but a FIZZLE is observable without one: a session
    // that errors, exhausts its steps, or "finishes" having changed nothing.
    // Solve times are heavy-tailed, so restart theory applies: banish the
    // fizzled context whole and try again under the other stars, with a
    // Fibonacci-longer budget. A session that edited files ends the spiral —
    // judgement then belongs to the user (or the gate, when there is one).
    let budgets = kaos::spiral::budgets(max_steps().max(14) + 7);
    let total_attempts = budgets.len();
    let mut n = 0;
    let mut intent = task.clone();
    let mut session_out: Option<kaos::conductor::Session> = None;
    for (attempt, &steps_budget) in budgets.iter().enumerate() {
        let polarity = kaos::spiral::Polarity::of_attempt(attempt);
        if attempt > 0 {
            println!(
                "  {}",
                ash(&format!(
                    "\u{2734} the working fizzled \u{2014} banished; the spiral turns ({} stars, {} steps)",
                    polarity.name(),
                    steps_budget
                )),
            );
        }
        let mut sampling =
            kaos::backend::Sampling::seeded(hash_str(&format!("spiral|{task}|{attempt}")));
        sampling.temperature = polarity.temperature();
        let session = run_session(
            &root_path,
            &intent,
            &spec,
            sampling,
            steps_budget,
            &mut |step| {
                n += 1;
                render_step(n, step);
            },
        );
        let fizzle = kaos::spiral::fizzled(&session);
        session_out = Some(session);
        if !fizzle || attempt + 1 == total_attempts {
            break;
        }
        // The Gnosis Crossing: both polarities of what the banished self
        // learned — the map it drew (positive) and the verdict (negative).
        let gnosis = session_out
            .as_ref()
            .map(kaos::spiral::gnosis)
            .unwrap_or_default();
        // The Dream (G12): a toolless, lunar divination over the banished
        // working seeds the next self with one hypothesis to test first.
        let dream = dream_between(&spec, &task, &gnosis);
        intent = format!(
            "{task}\n\nYou have attempted this before; that working was banished, but its gnosis crosses:\n{gnosis}{dream}\
             Use the map — go straight to what matters, make the change, verify it, then finish."
        );
    }
    let Some(session_out) = session_out else {
        eprintln!("kaos: the execution plan contained no attempts");
        return;
    };

    println!("{}", rule(64));
    if let Some(err) = &session_out.error {
        println!(
            "  {} {}",
            fg(RED(), "\u{2734} the Work falters \u{2014}"),
            ash(err)
        );
    } else if session_out.finished {
        println!(
            "  {}  {}",
            bold((90, 200, 110), "\u{2734} the Work is done"),
            ash(&session_out.final_message),
        );
    } else {
        println!(
            "  {}  {}",
            fg(RED(), "\u{2734} budget spent"),
            ash("the Work stands unfinished")
        );
    }
}

/// The verified best-of-k coding conclave: k adepts each work an isolated copy of the
/// target, a gate weighs each, and the consensus verified diff ships. Each adept's
/// full trace is emitted inside a **fold** so the reader sees a one-line verdict and
/// can expand the steps.
fn code_conclave(
    root: &std::path::Path,
    task: &str,
    k: usize,
    verify_cmd: Option<&str>,
    spec: &Spec,
) {
    use kaos::conductor::{run_conclave, ConclaveEvent};

    // k adepts, each a fresh conductor over the same mind. Diversity is *controlled*:
    // each adept samples with a distinct seed derived from the task and its index
    // (honoured on ollama minds, where the seed can actually be pinned), so the k
    // attempts genuinely differ yet the whole conclave reproduces from its task.
    // Every adept works a private copy, so their attempts never collide.
    let names = [
        "Frater Stokastikos",
        "Soror Bellona",
        "Frater Hermeticus",
        "Soror Genetrix",
        "Frater Tenebrae",
        "Soror Nox",
        "Frater Ludens",
    ];
    let adepts: Vec<(String, Box<dyn Chat>)> = (0..k)
        .map(|i| {
            let sampling = kaos::backend::Sampling::seeded(hash_str(&format!("{task}|{i}")));
            let chat: Box<dyn Chat> = Box::new(ProviderChat {
                spec: spec.clone(),
                timeout_s: 120,
                sampling: Some(sampling),
            });
            (names[i % names.len()].to_string(), chat)
        })
        .collect();

    let mut step_n = 0usize;
    let mut open_fold = false;
    let res = run_conclave(
        root,
        task,
        verify_cmd,
        adepts,
        max_steps(),
        120,
        |ev| match ev {
            ConclaveEvent::AdeptStart { i, k, name } => {
                step_n = 0;
                kaos::fold::open(&format!(
                    "\u{25c9} adept {}/{}  {}  \u{2014} working an isolated copy\u{2026}",
                    i + 1,
                    k,
                    name,
                ));
                open_fold = true;
            }
            ConclaveEvent::Step { i: _, step } => {
                step_n += 1;
                render_step(step_n, &step);
            }
            ConclaveEvent::AdeptEnd {
                i,
                verified,
                changed,
                diff_lines,
                note,
            } => {
                // Close the fold FIRST so the verdict prints at top level — the reader sees
                // each adept's outcome even with the step-detail fold collapsed.
                if open_fold {
                    kaos::fold::close();
                    open_fold = false;
                }
                let mark = if verified {
                    bold((90, 200, 110), "\u{2713} adept weighed true")
                } else {
                    fg(RED(), "\u{2717} adept false")
                };
                println!(
                    "  {} {}  {}",
                    mark,
                    dim(
                        ASH(),
                        &format!("[{}] {changed} file(s), +/- {diff_lines} lines", i + 1)
                    ),
                    dim(ASH(), &note),
                );
            }
            ConclaveEvent::Adjourned { convened, k } => {
                println!(
                "  {}  {}",
                bold(RED(), "\u{2734} the quorum adjourns"),
                dim(ASH(), &format!(
                    "the vote is beyond overturning after {convened}/{k} adepts \u{2014} the rest are never summoned"
                )),
            );
            }
            ConclaveEvent::Shipped {
                winner,
                votes,
                gated,
                files,
            } => {
                println!("{}", rule(64));
                let how = if gated {
                    "verified"
                } else {
                    "consensus-only (no gate)"
                };
                println!(
                    "  {}  {}",
                    bold((90, 200, 110), "\u{2734} SHIPPED"),
                    ash(&format!(
                    "adept {}'s {how} diff \u{2014} {votes}/{k} agreed, {files} file(s) written",
                    winner + 1,
                )),
                );
                if !gated {
                    println!("  {}", dim(ASH(), "no gate given: this is consensus, not verification. add `-- <test cmd>` to weigh it."));
                }
            }
            ConclaveEvent::NothingShipped { gated } => {
                println!("{}", rule(64));
                if gated {
                    println!(
                        "  {}  {}",
                        fg(RED(), "\u{2734} nothing ships"),
                        ash("no adept's diff passed the gate \u{2014} the project is untouched")
                    );
                } else {
                    println!(
                        "  {}  {}",
                        fg(RED(), "\u{2734} nothing ships"),
                        ash("no adept finished with a diff")
                    );
                }
            }
        },
    );

    if let Err(e) = res {
        if open_fold {
            kaos::fold::close();
        }
        println!(
            "  {} {}",
            fg(RED(), "\u{2734} the conclave falters \u{2014}"),
            ash(&e.to_string())
        );
    }
}

/// Render one agent step live and *compressed*: file edits become a two-line
/// `-/+` diff, writes a `+A -B` summary, and command runs show the command and a
/// condensed result. Everything is one-lined and truncated so it streams tightly.
const GREEN: (u8, u8, u8) = (90, 200, 110);
const REMOVE: (u8, u8, u8) = (210, 70, 70);
const AMBER: (u8, u8, u8) = (220, 150, 60);
const VIOLET: (u8, u8, u8) = (150, 130, 200);

fn render_step(n: usize, step: &Step) {
    let idx = dim(ASH(), &format!("{n:>2}"));
    // The adept's narration — what it says it is doing — streams LIVE above the
    // action, so the reader follows the work as it happens (the full text stays
    // in the step's fold). Two lines at most; the compact trace stays a trace.
    if !step.thought.is_empty() {
        for line in step
            .thought
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(2)
        {
            println!(
                "     {}",
                dim(VIOLET, &format!("\u{263d} {}", trunc_line(line.trim(), 92)))
            );
        }
    }
    match &step.tool {
        Tool::ReadFile { path } => {
            println!(
                "  {} {} {}",
                idx,
                fg(VIOLET, "\u{25cb} read"),
                dim(ASH(), path)
            );
        }
        Tool::EditFile {
            path,
            find,
            replace,
        } => {
            // The change, as a REAL diff streamed inline (bounded; full in the fold).
            println!("  {} {} {}", idx, bold(AMBER, "\u{00b1} edit"), bone(path));
            inline_diff(find, '-', REMOVE);
            inline_diff(replace, '+', GREEN);
        }
        Tool::WriteFile { path, contents } => {
            // The conductor's observation already carries "+A -B" / "N lines".
            println!(
                "  {} {} {}  {}",
                idx,
                bold(AMBER, "\u{271a} write"),
                bone(path),
                dim(GREEN, &one_line(step.observation.trim(), 60)),
            );
            // A created/rewritten file shows its head inline, like a diff.
            inline_diff(contents, '+', GREEN);
        }
        Tool::Bash { cmd } => {
            // Code execution: the command, then a condensed result.
            println!(
                "  {} {} {}",
                idx,
                bold(RED(), "$"),
                bone(&one_line(cmd, 84))
            );
            let (exit, tail) = bash_result(&step.observation);
            let colour = if exit == "exit 0" { GREEN } else { REMOVE };
            let line = if tail.is_empty() {
                exit.clone()
            } else {
                format!("{exit} \u{00b7} {tail}")
            };
            println!(
                "     {} {}",
                fg(colour, "\u{2192}"),
                dim(ASH(), &one_line(&line, 88))
            );
        }
        Tool::Finish { message } => {
            // The final message is the deliverable — as long as it needs to be.
            println!("  {} {}", idx, bold((90, 200, 110), "\u{2691} finish"));
            for line in message.lines().filter(|l| !l.trim().is_empty()) {
                println!("     {}", bone(line));
            }
        }
    }
    render_step_detail(n, step);
}

/// Stream up to 6 lines of a change block inline, opencode-style, with a count
/// of what remains (the complete text lives in the step's fold).
fn inline_diff(text: &str, sign: char, colour: (u8, u8, u8)) {
    const SHOWN: usize = 6;
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() && !text.is_empty() {
        println!(
            "     {}",
            fg(colour, &format!("{sign} {}", trunc_line(text, 88)))
        );
        return;
    }
    for l in lines.iter().take(SHOWN) {
        println!(
            "     {}",
            fg(colour, &format!("{sign} {}", trunc_line(l, 88)))
        );
    }
    if lines.len() > SHOWN {
        println!(
            "     {}",
            dim(
                ASH(),
                &format!(
                    "{sign} \u{2026} {} more lines (in the fold)",
                    lines.len() - SHOWN
                )
            )
        );
    }
}

/// The expandable truth of a step — what *really* happened, foldable in the TUI:
/// the model's complete visible text, the full diff, and the whole command output.
/// Emitted only under the fold protocol (the TUI); the plain CLI keeps its compact
/// trace above. No limits belong here: the run browser owns scrolling and wrapping.
fn render_step_detail(n: usize, step: &Step) {
    if !kaos::fold::enabled() {
        return;
    }
    let title = match &step.tool {
        Tool::ReadFile { path } => format!("read {path}"),
        Tool::EditFile { path, .. } => format!("edit {path}"),
        Tool::WriteFile { path, .. } => format!("write {path}"),
        Tool::Bash { cmd } => format!("$ {}", one_line(cmd, 48)),
        Tool::Finish { .. } => "finish".to_string(),
    };
    kaos::fold::open(&dim(
        ASH(),
        &format!("    \u{22ef} step {n} in full \u{2014} {title}"),
    ));
    if !step.thought.is_empty() {
        println!("  {}", fg(VIOLET, "\u{263d} complete model text:"));
        for l in step.thought.lines() {
            println!("    {}", dim(ASH(), l));
        }
    }
    match &step.tool {
        Tool::EditFile { find, replace, .. } => {
            println!("  {}", ash("the exact change:"));
            for l in find.lines() {
                println!("    {}", fg(REMOVE, &format!("- {l}")));
            }
            for l in replace.lines() {
                println!("    {}", fg(GREEN, &format!("+ {l}")));
            }
        }
        Tool::WriteFile { contents, .. } => {
            println!(
                "  {}",
                ash(&format!(
                    "the new contents ({} lines):",
                    contents.lines().count()
                ))
            );
            for l in contents.lines() {
                println!("    {}", fg(GREEN, &format!("+ {l}")));
            }
        }
        Tool::Bash { .. } => {
            println!("  {}", ash("the full output:"));
            for l in step.observation.lines() {
                println!("    {}", dim(ASH(), l));
            }
        }
        Tool::ReadFile { .. } => {
            println!("  {}", ash("what it saw:"));
            for l in step.observation.lines() {
                println!("    {}", dim(ASH(), l));
            }
        }
        Tool::Finish { message } => {
            println!("  {}", ash(&format!("declared done: {message}")));
        }
    }
    kaos::fold::close();
}

/// Collapse a (possibly multi-line) string to a single truncated line, marking line
/// breaks with ⏎ so a compressed diff still reads.
fn one_line(s: &str, n: usize) -> String {
    let marked = s.replace('\n', " \u{21b5} ");
    let collapsed = marked.split_whitespace().collect::<Vec<_>>().join(" ");
    trunc_line(&collapsed, n)
}

/// Split a bash observation ("exit N\n<output>") into (exit line, last output line).
fn bash_result(obs: &str) -> (String, String) {
    let lines: Vec<&str> = obs.lines().collect();
    let exit = lines.first().copied().unwrap_or("").to_string();
    let tail = lines
        .iter()
        .skip(1)
        .rev()
        .find(|l| !l.trim().is_empty())
        .copied()
        .unwrap_or("")
        .to_string();
    (exit, tail)
}

fn trunc_line(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// /forge — the Conclave doing REAL agentic work, gated by REAL tests.
/// Creates a broken demo project, convenes k adepts (each edits an isolated copy),
/// runs the project's tests as the Ma'at gate, and ships the consensus VERIFIED fix.
/// Usage: forge [offline|ollama[:model]|claude] [k]
fn forge_cmd(arg: &str) {
    let mut parts = arg.split_whitespace();
    let backend = parts.next().unwrap_or("offline");
    let k: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(3);

    // Build the arena in a temp dir.
    let dir = std::env::temp_dir().join(format!("kaos-forge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (foi, verify) = match agent::write_demo_arena(&dir) {
        Ok(x) => x,
        Err(e) => {
            println!("{}", fg(RED(), &format!("could not raise the arena: {e}")));
            return;
        }
    };
    let foi_refs: Vec<&str> = foi.iter().map(|s| s.as_str()).collect();
    let before = std::fs::read_to_string(dir.join("sol.py")).unwrap_or_default();

    println!();
    println!(
        "  {}  {}",
        bold(RED(), "FORGE"),
        dim(
            ASH(),
            "the Conclave mends real files; the tests weigh the heart"
        )
    );
    println!(
        "  {} {}",
        ash("arena"),
        dim(ASH(), &dir.display().to_string())
    );
    println!("  {} {}", ash("wound"), bone("add(a,b) is broken"));
    println!(
        "  {} {}",
        ash("feather"),
        dim(
            ASH(),
            "the tests must pass \u{2014} nothing unweighed ships"
        )
    );
    println!("  {} {}", ash("before"), fg(RED(), before.trim()));
    println!("{}", rule(64));

    // Convene the adepts.
    let names = [
        "Frater Stokastikos",
        "Soror Bellona",
        "Frater Hermeticus",
        "Soror Genetrix",
        "Frater Tenebrae",
    ];
    let agents: Vec<Box<dyn AdeptAgent>> = match backend {
        "offline" | "sim" => {
            // Deterministic, no model: every adept proposes the known fix.
            (0..k)
                .map(|i| {
                    Box::new(ScriptedAgent {
                        name: names[i % names.len()].to_string(),
                        patch: agent::demo_fix_patch(),
                    }) as Box<dyn AdeptAgent>
                })
                .collect()
        }
        b => {
            let mb = if let Some(model) = b.strip_prefix("ollama:") {
                ModelBackend::Ollama {
                    model: model.to_string(),
                    timeout_s: 60,
                }
            } else if b == "ollama" {
                ModelBackend::Ollama {
                    model: "qwen2.5:3b".to_string(),
                    timeout_s: 60,
                }
            } else if b == "claude" {
                ModelBackend::Claude { model: None }
            } else {
                println!("{}", ash("backend: offline | ollama[:model] | claude"));
                return;
            };
            println!(
                "  {}",
                dim(ASH(), &format!("summoning {k} adepts through {b}\u{2026}"))
            );
            (0..k)
                .map(|i| {
                    Box::new(ModelAgent {
                        name: names[i % names.len()].to_string(),
                        backend: mb.clone(),
                        edit_path: "sol.py".to_string(),
                    }) as Box<dyn AdeptAgent>
                })
                .collect()
        }
    };

    let mut rng = Rng::new(seed_from_clock());
    let verdict = match agent::solve(&dir, "fix add(a,b)", &foi_refs, &verify, &agents, &mut rng) {
        Ok(v) => v,
        Err(e) => {
            println!("{}", fg(RED(), &format!("the Work falters \u{2014} {e}")));
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
    };

    // Each adept's verdict at the scales.
    for a in &verdict.attempts {
        let mark = if a.passed {
            bold((90, 200, 110), "weighed true \u{2713}")
        } else {
            fg(RED(), "false \u{2717}")
        };
        println!(
            "  {:<20} {}  {}",
            bold(RED(), &a.adept),
            mark,
            dim(ASH(), &a.note)
        );
    }
    println!("{}", rule(64));

    if verdict.shipped {
        let after = std::fs::read_to_string(dir.join("sol.py")).unwrap_or_default();
        println!(
            "  {}  {} {}",
            bold((90, 200, 110), "\u{2734} SHIPPED"),
            dim(
                ASH(),
                &format!("{}/{} weighed true", verdict.passed, verdict.k)
            ),
            ash("\u{2014} the consensus fix"),
        );
        println!("  {} {}", ash("after"), bone(after.trim()));
        // The scales, once more, on what shipped.
        let final_ok = std::process::Command::new("sh")
            .arg("-c")
            .arg(&verify)
            .current_dir(&dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        println!(
            "  {} {}",
            ash("feather:"),
            if final_ok {
                bold((90, 200, 110), "true")
            } else {
                fg(RED(), "false")
            }
        );
    } else {
        println!(
            "  {}  {}",
            fg(RED(), "\u{2734} nothing ships"),
            ash(&format!(
                "0/{} weighed true \u{2014} the gate holds",
                verdict.k
            )),
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// /models — every mind the Pact can summon, grouped by provider, with the
/// exact /model incantation for each and the current binding marked.
/// `/models openrouter` fetches and lists the full live OpenRouter catalog.
fn models_cmd(session: &Session, arg: &str) {
    let bound = session.model.canonical();
    let mark = |c: &str| {
        if c == bound {
            bold(RED(), "\u{2734} ")
        } else {
            "  ".to_string()
        }
    };
    if matches!(arg.trim().to_lowercase().as_str(), "openrouter" | "router") {
        return openrouter_catalog(&mark);
    }
    println!();
    println!(
        "  {}",
        bold(RED(), "THE MINDS \u{2014} every model the Pact can summon")
    );
    println!(
        "  {}",
        dim(
            ASH(),
            &format!(
                "bound now: {}   (bind any line with /model <token>)",
                session.model.label()
            )
        )
    );
    println!("{}", rule(64));

    println!(
        "  {}",
        ash("claude CLI \u{2014} your Claude subscription (no API key)")
    );
    for (tok, desc) in [
        ("claude", "the CLI's own default model"),
        (
            "claude:sonnet",
            "Sonnet \u{2014} fast, strong; the everyday adept",
        ),
        (
            "claude:opus",
            "Opus \u{2014} deepest; the Magus, spend it where it counts",
        ),
        ("claude:haiku", "Haiku \u{2014} cheapest and quickest"),
        (
            "claude:fable",
            "Fable \u{2014} selected through the Claude CLI subscription",
        ),
    ] {
        println!(
            "  {}{}  {}",
            mark(tok),
            bold(RED(), &format!("{tok:<24}")),
            dim(ASH(), desc)
        );
    }

    let keyed = |var: &str| std::env::var(var).ok().filter(|v| !v.is_empty()).is_some();
    println!();
    println!(
        "  {} {}",
        ash("anthropic API"),
        if keyed("ANTHROPIC_API_KEY") {
            dim(ASH(), "\u{2014} ANTHROPIC_API_KEY set")
        } else {
            fg(RED(), "\u{2014} needs ANTHROPIC_API_KEY")
        },
    );
    for tok in [
        "anthropic:claude-sonnet-4-5",
        "anthropic:claude-opus-4-8",
        "anthropic:claude-haiku-4-5",
    ] {
        println!(
            "  {}{}",
            mark(&Spec::parse(tok).canonical()),
            dim(ASH(), tok)
        );
    }

    println!();
    println!(
        "  {} {}",
        ash("openai API"),
        if keyed("OPENAI_API_KEY") {
            dim(ASH(), "\u{2014} OPENAI_API_KEY set")
        } else {
            fg(
                RED(),
                "\u{2014} needs OPENAI_API_KEY (OPENAI_BASE_URL for compatibles)",
            )
        },
    );
    for tok in ["openai:gpt-4o", "openai:gpt-4o-mini", "openai:o3-mini"] {
        println!(
            "  {}{}",
            mark(&Spec::parse(tok).canonical()),
            dim(ASH(), tok)
        );
    }

    println!();
    println!(
        "  {} {}",
        ash("openrouter \u{2014} one key, every hosted model"),
        if keyed("OPENROUTER_API_KEY") {
            dim(ASH(), "\u{2014} OPENROUTER_API_KEY set")
        } else {
            fg(RED(), "\u{2014} needs OPENROUTER_API_KEY")
        },
    );
    for (tok, desc) in [
        (
            "openrouter:openrouter/auto",
            "the router picks the model per request",
        ),
        (
            "openrouter:anthropic/claude-sonnet-4.5",
            "Claude through the router",
        ),
        (
            "openrouter:deepseek/deepseek-chat",
            "DeepSeek V3 \u{2014} strong and cheap",
        ),
        (
            "openrouter:meta-llama/llama-3.3-70b-instruct",
            "open weights, hosted",
        ),
    ] {
        println!(
            "  {}{}  {}",
            mark(&Spec::parse(tok).canonical()),
            dim(ASH(), &format!("{tok:<44}")),
            dim(ASH(), desc)
        );
    }
    println!(
        "  {}",
        dim(ASH(), "  /models openrouter \u{2014} the full live catalog")
    );

    println!();
    println!(
        "  {}",
        ash("ollama \u{2014} local, free, seeded sampling + thinking control")
    );
    match std::process::Command::new("ollama").arg("list").output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut any = false;
            for name in text
                .lines()
                .skip(1)
                .filter_map(|l| l.split_whitespace().next())
            {
                let tok = format!("ollama:{name}");
                println!("  {}{}", mark(&tok), dim(ASH(), &tok));
                any = true;
            }
            if !any {
                println!(
                    "  {}",
                    dim(ASH(), "  (none pulled \u{2014} `ollama pull qwen3:14b`)")
                );
            }
        }
        Err(_) => println!("  {}", dim(ASH(), "  (ollama not installed)")),
    }

    println!();
    println!(
        "  {}{}  {}",
        mark("sim"),
        bold(RED(), &format!("{:<24}", "sim")),
        dim(
            ASH(),
            "offline simulation \u{2014} the equation decides, no model"
        )
    );
    println!("{}", rule(64));
}

/// /models openrouter — the full live catalog from openrouter.ai, with context
/// window and prompt price, each line a bindable /model token.
#[cfg(feature = "api")]
fn openrouter_catalog(mark: &dyn Fn(&str) -> String) {
    println!();
    println!(
        "  {}",
        bold(RED(), "THE ROUTER \u{2014} every mind behind openrouter.ai")
    );
    print!("  {}", dim(ASH(), "consulting the catalog\u{2026}"));
    io::stdout().flush().ok();
    let models = match kaos::provider::openrouter_models(std::time::Duration::from_secs(15)) {
        Ok(m) => m,
        Err(e) => {
            println!(
                "\r  {}",
                fg(RED(), &format!("\u{2734} the catalog would not open: {e}"))
            );
            return;
        }
    };
    println!(
        "\r  {}",
        dim(
            ASH(),
            &format!(
                "{} models \u{2014} bind any with /model openrouter:<id>          ",
                models.len()
            )
        ),
    );
    println!("{}", rule(64));
    for m in &models {
        let ctx = if m.context > 0 {
            format!("{}k", m.context / 1000)
        } else {
            "?".to_string()
        };
        let price = match m.prompt_per_m {
            Some(0.0) => "free".to_string(),
            Some(p) => format!("${p:.2}/M"),
            None => "\u{2014}".to_string(),
        };
        let tok = format!("openrouter:{}", m.id);
        println!(
            "  {}{}  {}",
            mark(&tok),
            dim(ASH(), &format!("{:<52}", m.id)),
            dim(ASH(), &format!("{ctx:>6}  {price:>9}"))
        );
    }
    println!("{}", rule(64));
}

#[cfg(not(feature = "api"))]
fn openrouter_catalog(_mark: &dyn Fn(&str) -> String) {
    println!(
        "  {}",
        ash("built without the `api` feature \u{2014} the router is out of reach.")
    );
}

/// /model — bind the mind the rites summon. `/model` alone reveals the current
/// binding, the reachable providers, and local ollama models. Set with a provider
/// name (with optional model) or a bare model tag:
///   sim · claude[:sonnet|opus|haiku|fable] · openai[:gpt-4o] · anthropic[:model] · ollama[:model] · gpt-4o …
fn model_cmd(session: &mut Session, arg: &str) {
    let arg = arg.trim();
    if arg.is_empty() {
        println!("  {} {}", ash("bound"), bold(RED(), &session.model.label()));
        // Which HTTP providers hold a key.
        let mut ready = Vec::new();
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            ready.push("anthropic");
        }
        if std::env::var("OPENAI_API_KEY").is_ok() {
            ready.push("openai");
        }
        if std::env::var("OPENROUTER_API_KEY").is_ok() {
            ready.push("openrouter");
        }
        ready.push("claude(cli)");
        println!("  {} {}", ash("keyed"), dim(ASH(), &ready.join("  ")));
        if let Ok(out) = std::process::Command::new("ollama").arg("list").output() {
            let text = String::from_utf8_lossy(&out.stdout);
            let models: Vec<&str> = text
                .lines()
                .skip(1)
                .filter_map(|l| l.split_whitespace().next())
                .collect();
            if !models.is_empty() {
                println!("  {} {}", ash("local"), dim(ASH(), &models.join("  ")));
            }
        }
        println!("  {}", dim(ASH(), "bind: /model claude[:sonnet|opus|haiku|fable] | openai[:gpt-4o] | anthropic[:model] | openrouter[:vendor/model] | ollama:qwen2.5:3b | sim"));
        return;
    }
    // A provider + optional model given as two words ("openai gpt-4o") folds into
    // the colon form parse understands.
    let spec = Spec::parse(&arg.replacen(' ', ":", 1));
    let warn = spec.readiness().err();
    let canonical = spec.canonical();
    session.model = spec;
    println!("  {} {}", ash("bound"), bold(RED(), &session.model.label()));
    // The fullscreen app handles `/model` locally; this path covers the classic
    // prompt and `kaos model …`. Tests must never rewrite the developer's config.
    if cfg!(not(test)) {
        match kaos::config::set_value("KAOS_MODEL", &canonical) {
            Ok(path) => println!(
                "  {} {}",
                ash("remembered"),
                dim(ASH(), &path.display().to_string())
            ),
            Err(error) => println!("  {} {}", fg(RED(), "✴ could not remember"), ash(&error)),
        }
    }
    if let Some(w) = warn {
        println!(
            "  {} {}",
            fg(RED(), "\u{2734} but"),
            ash(&format!(
                "{w} \u{2014} the mind will not answer until it is set"
            ))
        );
    }
}

/// /banish — laughter scatters the work. The Pact reconvenes from nothing.
fn banish_session(session: &mut Session) {
    session.pact = Pact::convene();
    println!("  {}", bold(RED(), "HA HA HA \u{2014} banished."));
    println!(
        "  {}",
        dim(
            ASH(),
            "grades, egregore, context \u{2014} scattered to the floor."
        )
    );
}

// ──────────────────────────── rendering ────────────────────────────

fn render_rite(rite: &Rite) {
    println!();
    println!("{}", rule(62));
    println!("  {} {}", bold(RED(), "RITE"), bone(&rite.task));
    render_sigil_block(&rite.sigil, rite.ray);
    println!(
        "  {}",
        dim(ASH(), &format!("statement of intent: {}", rite.statement))
    );
    println!("{}", dim(OXBLOOD(), &"\u{2508}".repeat(62)));

    for (n, att) in rite.attempts.iter().enumerate() {
        let verb = if att.charge.fired {
            bold((90, 200, 110), "CHARGED TRUE")
        } else {
            fg(RED(), "fizzled")
        };
        println!(
            "  {} {} {} {}",
            dim(ASH(), &format!("life {}", n + 1)),
            bold(rite.ray.rgb(), &format!("{:<18}", att.adept_name)),
            dim(ASH(), &format!("[{}]", att.charge.current.name())),
            verb,
        );
        render_equation(&att.charge.eq, att.charge.current);
        println!(
            "      {} {}",
            ash("M ="),
            bold(RED(), &format!("{:.1}%", att.charge.magic_factor * 100.0)),
        );
        if !att.charge.fired && n + 1 < rite.attempts.len() {
            println!(
                "      {}",
                dim(
                    ASH(),
                    "banish \u{2192} laughter \u{2192} R reset, paradigm shift to the next adept"
                ),
            );
        }
    }

    println!("{}", dim(OXBLOOD(), &"\u{2508}".repeat(62)));
    if let (true, Some(last)) = (rite.succeeded, rite.final_attempt()) {
        println!(
            "  {}  {} {} {}",
            bold((90, 200, 110), "\u{2734} WEIGHED TRUE \u{2014} shipped"),
            ash("by"),
            bold(rite.ray.rgb(), &last.adept_name),
            dim(ASH(), &format!("on life {}", rite.attempts.len())),
        );
    } else {
        println!(
            "  {}  {}",
            fg(RED(), "\u{2734} the heart was not weighed true"),
            ash("\u{2014} the work was not shipped (no false positives pass the gate)"),
        );
    }
    println!("{}", rule(62));
}

/// The sigil block: glyph, residue, compression, and the awareness it buys.
fn render_sigil_block(sigil: &Sigil, ray: Ray) {
    println!(
        "  {} {}   {} {}",
        ash("ray"),
        bold(
            ray.rgb(),
            &format!("{} \u{2014} {}", ray.name(), ray.sphere())
        ),
        ash("sigil"),
        bold(RED(), &sigil.glyph()),
    );
    println!(
        "  {} {}   {} {}   {} {}",
        ash("residue"),
        bone(&sigil.residue.iter().collect::<String>()),
        ash("compressed"),
        bone(&format!("{:.0}%", sigil.compression() * 100.0)),
        ash("\u{2192} A ="),
        bold(RED(), &format!("{:.2}", sigil.awareness())),
    );
}

/// The four factors of Carroll's equation, in a single line, red for emphasis.
fn render_equation(eq: &kaos::equation::Equation, current: Current) {
    let cur = match current {
        Current::Inhibitory => "still",
        Current::Excitatory => "ecstatic",
    };
    println!(
        "      {} {}={} {}={} {}={} {}={}  {}",
        dim(ASH(), "M = G\u{b7}L\u{b7}(1-A)\u{b7}(1-R)"),
        ash("G"),
        bone(&format!("{:.2}", eq.gnosis)),
        ash("L"),
        bone(&format!("{:.2}", eq.link)),
        ash("A"),
        bone(&format!("{:.2}", eq.awareness)),
        ash("R"),
        bone(&format!("{:.2}", eq.resistance)),
        dim(ASH(), &format!("({cur})")),
    );
}

// ──────────────────────────── chrome ───────────────────────────────

fn banner(session: &Session) {
    println!();
    println!("{}", chaos_star_red());
    println!();
    println!(
        "  {}  {}",
        bold(RED(), "\u{2734} kaos"),
        dim(ASH(), "\u{2014} the Pact convenes."),
    );
    println!(
        "  {}",
        ash("sigil lowers A \u{b7} routing raises G \u{b7} banishing lowers R \u{b7} egregore raises L"),
    );
    println!(
        "  {}  {}",
        dim(ASH(), &format!("mind: {}", session.model.label())),
        dim(
            ASH(),
            "speak an intent \u{2014} the adept works these files \u{b7} /help"
        ),
    );
    println!();
}

fn print_help() {
    println!();
    println!("  {}", bold(RED(), "GRIMOIRE \u{2014} commands"));
    let cmds: &[(&str, &str)] = &[
        ("/cast <intent>", "cast the Work (or just speak it)"),
        (
            "/scry <intent>",
            "read the ray, sigil, and equation — no charge",
        ),
        (
            "/conclave <intent>",
            "run the default myth — a voted best-of-k — on the bound mind",
        ),
        (
            "/rebis [file]",
            "open the o-[]-o model-interface editor and live graph",
        ),
        (
            "/visual",
            "draw the o-[]-o mandala on a canvas; it writes the Rebis",
        ),
        (
            "/attach <file>",
            "add a file's contents as context (or drag-and-drop paths)",
        ),
        ("/roster", "the Pact, by grade"),
        ("/egregore", "the shared mind — its potency and lessons"),
        ("/rays", "the eight magics and their spheres"),
        (
            "/code [dir] <intent>",
            "the adept works; the gate is DIVINED and the quorum sizes itself",
        ),
        (
            "/code [dir] xK <intent> -- <test>",
            "manual override: fixed conclave of K, only the gated fix ships",
        ),
        (
            "/forge [sim|ollama|claude] [k]",
            "the Conclave mends code, weighed by the tests",
        ),
        (
            "/auth [provider [key]]",
            "store a provider API key (0600); no args shows status; `claude` explains login",
        ),
        (
            "/model [provider[:model]]",
            "bind the mind — claude[:sonnet|opus] · openai · anthropic · ollama · sim",
        ),
        (
            "/models",
            "every mind the Pact can summon, and how to bind each",
        ),
        ("/banish", "laughter — scatter grades, egregore, context"),
        ("/help", "this grimoire"),
        ("/quit", "close the temple"),
    ];
    for (c, d) in cmds {
        println!("    {}  {}", bold(RED(), &format!("{:<32}", c)), ash(d));
    }
    println!();
    println!(
        "  {}",
        dim(
            ASH(),
            "M = G\u{b7}L\u{b7}(1-A)\u{b7}(1-R) \u{2014} Carroll, Liber Kaos"
        ),
    );
}

fn print_rays() {
    println!();
    println!("  {}", bold(RED(), "THE EIGHT MAGICS \u{2014} Liber Kaos"));
    for ray in Ray::all() {
        println!(
            "    {} {}",
            bold(ray.rgb(), &format!("{:<9}", ray.name())),
            ash(ray.sphere()),
        );
    }
}

// ──────────────────────────── helpers ──────────────────────────────

fn seed_from_clock() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(2026)
}
