//! Shared prompt contracts for every Kaos conversation.
//!
//! A chat is not only a string sent to a provider. It needs a stable language
//! reference, a way to carry prior turns, and—when discussing execution—a
//! complete run snapshot. Keeping those rules here prevents the terminal,
//! visual editor, sigil supervisor, and run chat from slowly developing
//! incompatible prompt dialects.

use std::borrow::Cow;
use std::fmt::Write as _;

/// The auditable Rebis authoring and testing reference available to explicit
/// documentation tooling. It is never automatically appended to a provider
/// prompt or a Rebis node.
pub const REBIS_AUTHORING_GUIDE: &str = include_str!("../../docs/REBIS_CHAT_CONTEXT.md");
/// Prior conversation carried into a new provider request. Durable sessions
/// retain every turn; only the transient context window is bounded.
pub const MAX_CHAT_HISTORY_BYTES: usize = 256 * 1024;

/// The mark a stopped turn carries, so history can tell it from an answer.
pub const STOPPED_MARK: &str = "\u{25a0} stopped";

/// What a turn that produced nothing is recorded as. It is a note to the
/// reader, not something the model said, so it never re-enters a prompt.
pub const EMPTY_TURN_NOTICE: &str = "(the task ended without output)";

/// What a stored model turn contributes to the history sent to a model —
/// `None` when it contributes nothing.
///
/// A saved conversation keeps everything, including turns that are the HOST
/// speaking rather than the model: a provider failure, or a turn the reader
/// stopped. Those belong on screen — they are what happened — but sending them
/// back is a mistake with teeth. Each failed turn is stored, re-sent, and
/// re-fails, so a conversation that hits one error carries every error it has
/// ever hit into the next prompt: the history grows with text nobody wrote,
/// crowding out the real conversation and teaching the model that transcripts
/// contain error messages. Seen in the wild as a chat whose whole context had
/// become `ASSISTANT: chat error: oracle failure: …` repeated.
///
/// So an error turn is dropped, and a stopped turn keeps the part the model
/// actually wrote with the host's mark cut off the end.
#[must_use]
pub fn model_turn_for_history(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.starts_with("chat error:")
        || trimmed.starts_with("oracle failure:")
        || trimmed == EMPTY_TURN_NOTICE
    {
        return None;
    }
    let said = match trimmed.find(STOPPED_MARK) {
        Some(at) => trimmed[..at].trim_end(),
        None => trimmed,
    };
    (!said.is_empty()).then(|| said.to_string())
}

/// Render the complete host data envelope available to one Rebis node.
///
/// Rebis already carries structural context through `+`, arrows, mediators,
/// ports, and the record. This envelope carries the immutable run facts that
/// are outside the language value itself—source, initial record/input, scope,
/// selected model, workspace, directive metadata, and attachment metadata—so
/// every provider-backed agent sees the same inspectable context. It is data,
/// not a system instruction: the actual node prompt remains a separate,
/// length-delimited field at the end.
#[derive(Clone, Copy, Debug)]
pub struct RebisAgentContext<'a> {
    pub source: &'a str,
    pub record: &'a str,
    pub scope: &'a str,
    pub model: &'a str,
    pub workspace: &'a str,
    pub directive: Option<&'a str>,
    pub attachments: &'a [(&'a str, &'a str)],
    pub node_prompt: &'a str,
}

#[must_use]
pub fn render_rebis_agent_context(context: RebisAgentContext<'_>) -> String {
    let mut rendered = String::from("KAOS_REBIS_AGENT_CONTEXT\nVERSION=1\n");
    append_context_field(&mut rendered, "SCOPE", context.scope);
    append_context_field(&mut rendered, "MODEL", context.model);
    append_context_field(&mut rendered, "WORKSPACE", context.workspace);
    append_context_field(&mut rendered, "PROGRAM", context.source);
    append_context_field(&mut rendered, "INITIAL_RECORD", context.record);
    append_context_field(
        &mut rendered,
        "SUPERVISOR_DIRECTIVE",
        context.directive.unwrap_or(""),
    );
    let _ = writeln!(rendered, "ATTACHMENT_COUNT={}", context.attachments.len());
    for (index, (name, media_type)) in context.attachments.iter().enumerate() {
        append_context_field(&mut rendered, &format!("ATTACHMENT_{index}_NAME"), name);
        append_context_field(
            &mut rendered,
            &format!("ATTACHMENT_{index}_MEDIA_TYPE"),
            media_type,
        );
    }
    append_context_field(&mut rendered, "NODE_PROMPT", context.node_prompt);
    rendered
}

fn append_context_field(rendered: &mut String, name: &str, value: &str) {
    let _ = writeln!(rendered, "{name}_BYTES={}", value.len());
    rendered.push_str(value);
    rendered.push('\n');
}

/// The shared context handle used by terminal, visual, supervisory, and
/// reusable agent chats. It is deliberately a zero-sized capability marker:
/// the host must not inject documentation or instructions into a user's data.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChatContext;

impl ChatContext {
    /// The standard Kaos context, backed by the checked-in Rebis guide.
    #[must_use]
    pub const fn rebis() -> Self {
        Self
    }

    /// The checked-in reference for callers that explicitly render help.
    #[must_use]
    pub const fn guide(self) -> &'static str {
        REBIS_AUTHORING_GUIDE
    }

    /// Preserve the provider's system contract exactly.
    ///
    /// This method remains as a compatibility seam for frontends that share a
    /// context object, but it does not augment the contract. Rebis knowledge is
    /// supplied by actual Rebis source or explicit user input, never hidden
    /// prompt text.
    #[must_use]
    pub fn augment_system(self, system: &str) -> String {
        system.to_string()
    }

    /// Preserve the provider's system contract exactly.
    ///
    /// Separate from [`ChatContext::augment_system`] so a host that has no
    /// library, or does not want a conversation writing to one, keeps the
    /// reading-only contract by construction rather than by remembering to
    /// strip something. Granting the capability is a call the host makes.
    #[must_use]
    pub fn augment_system_with_library(self, system: &str) -> String {
        self.augment_system(system)
    }

    /// Render an ordinary conversation turn.
    ///
    /// This is a data envelope, not an instruction envelope. Length fields make
    /// the boundary inspectable and keep user/model history from becoming a
    /// host-authored directive.
    #[must_use]
    pub fn render_chat(self, history: &str, question: &str) -> String {
        let history = bounded_tail(
            history,
            MAX_CHAT_HISTORY_BYTES,
            "[... earlier conversation omitted ...]\n",
        );
        format!(
            "KAOS_CHAT_DATA\nMODE=direct\nHISTORY_BYTES={}\n{}\nQUESTION_BYTES={}\n{}\n",
            history.len(),
            if history.trim().is_empty() {
                "(first turn)"
            } else {
                history.as_ref()
            },
            question.len(),
            question
        )
    }

    /// Render the same data boundary with a mode marker for the actual Rebis
    /// composer. The composer itself is built by [`crate::chaos::composition_source`].
    #[must_use]
    pub fn augment_system_for_chaos(self, system: &str) -> String {
        self.augment_system(system)
    }

    /// Render a conversation turn that is to be answered with a program.
    ///
    /// The history and intent remain data. No host-authored natural-language
    /// composer contract is smuggled into the request.
    #[must_use]
    pub fn render_chaos_chat(self, history: &str, question: &str) -> String {
        let history = bounded_tail(
            history,
            MAX_CHAT_HISTORY_BYTES,
            "[... earlier conversation omitted ...]\n",
        );
        format!(
            "KAOS_CHAT_DATA\nMODE=chaos\nHISTORY_BYTES={}\n{}\nINTENT_BYTES={}\n{}\n",
            history.len(),
            if history.trim().is_empty() {
                "(first turn)"
            } else {
                history.as_ref()
            },
            question.len(),
            question
        )
    }

    /// Whichever of [`ChatContext::render_chat`] and
    /// [`ChatContext::render_chaos_chat`] the stance calls for.
    ///
    /// Every frontend renders its turn through this one call, so adopting chaos
    /// mode is a single boolean at the call site rather than a branch each
    /// surface has to remember to write. The terminal app, the visual editor
    /// and `kaos code` all reach it.
    #[must_use]
    pub fn render_turn(self, chaos: bool, history: &str, question: &str) -> String {
        if chaos {
            self.render_chaos_chat(history, question)
        } else {
            self.render_chat(history, question)
        }
    }

    /// Render a question about a live, paused, queued, or completed run.
    #[must_use]
    pub fn render_run(
        self,
        snapshot: &RunSnapshot<'_>,
        history: &[(String, String)],
        question: &str,
    ) -> String {
        render_run(snapshot, history, question)
    }
}

/// The default context handle used by frontends and agents.
pub const DEFAULT_CONTEXT: ChatContext = ChatContext::rebis();

/// The provider-independent facts about one retained Rebis evaluation.
/// Frontends own process handles and timers; this view owns the prompt shape.
#[derive(Clone, Copy, Debug)]
pub struct RunSnapshot<'a> {
    pub id: u64,
    pub state: &'a str,
    pub paused: bool,
    pub pause_reason: &'a str,
    pub scope: &'a str,
    pub mode: &'a str,
    pub lane: &'a str,
    pub timer: &'a str,
    pub child: &'a str,
    pub source: &'a str,
    pub input: &'a str,
    pub output: &'a [String],
}

/// Wrap an ordinary conversational turn with the same Rebis knowledge used by
/// run supervision. `history` is already rendered by the caller so the core
/// does not need to own a session store or a frontend transcript type.
#[must_use]
pub fn render_chat_prompt(history: &str, question: &str) -> String {
    DEFAULT_CONTEXT.render_chat(history, question)
}

/// Turn one already-rendered chat data envelope into an executable Rebis
/// prompt. The envelope remains a quoted prompt operand, so user/model text can
/// contain arbitrary delimiters without becoming Rebis syntax.
#[must_use]
pub fn chat_program_source(data: &str) -> String {
    rebis_lang::format(&rebis_lang::Expr::Prompt(data.to_string()))
}

/// Render a question about a live, paused, queued, or completed run. The
/// complete retained output is deliberately included as data; callers must not
/// trim it just because the run is finished or because the UI is currently
/// scrolled.
#[must_use]
pub fn render_run_chat(
    snapshot: &RunSnapshot<'_>,
    history: &[(String, String)],
    question: &str,
) -> String {
    DEFAULT_CONTEXT.render_run(snapshot, history, question)
}

/// Render the retained run itself for a frontend chat transcript.
///
/// This is deliberately free of model instructions. It is the same evidence
/// that [`render_run_chat`] carries in a provider prompt, but exposed as a
/// readable block so opening a run chat does not hide the source and output
/// that the question is about.
#[must_use]
pub fn render_run_snapshot(snapshot: &RunSnapshot<'_>) -> String {
    let output = if snapshot.output.is_empty() {
        "(no retained output yet)".to_string()
    } else {
        snapshot.output.join("\n")
    };
    format!(
        "RUN #{}\nstate: {}\npaused: {}\npause reason: {}\nscope: {}\nmode: {}\nlane: {}\ntimer: {}\nresident child: {}\n\nSOURCE\n{}\n\nRECORD / INPUT\n{}\n\nFULL RETAINED OUTPUT\n{}",
        snapshot.id,
        snapshot.state,
        snapshot.paused,
        snapshot.pause_reason,
        snapshot.scope,
        snapshot.mode,
        snapshot.lane,
        snapshot.timer,
        snapshot.child,
        snapshot.source,
        if snapshot.input.is_empty() {
            "(empty)"
        } else {
            snapshot.input
        },
        output,
    )
}

/// Remove terminal presentation from text before it becomes durable chat
/// history. Child processes may emit CSI colors, OSC hyperlinks, carriage
/// returns, or a visible `␛` representation when a terminal renderer is not
/// present; none of those belong in a model's next conversational turn.
#[must_use]
pub fn clean_chat_reply(text: &str) -> String {
    strip_terminal_escapes(text).trim().to_string()
}

/// Extract the answer from a streamed chat child.
///
/// Interactive front ends ask the child to emit its visible work as foldable
/// sections before a `chat    final answer` line. Those sections belong in the
/// live transcript, not in the durable conversation history. Older/final-only
/// children do not emit that delimiter, so the fallback keeps their complete
/// output compatible.
#[must_use]
pub fn extract_chat_reply(text: &str) -> String {
    let mut final_answer = Vec::new();
    let mut fallback = Vec::new();
    let mut collecting_final = false;
    let mut saw_final = false;
    let mut saw_fold = false;
    let mut saw_chat_line = false;

    for raw in text.lines() {
        match crate::fold::classify(raw) {
            crate::fold::Marker::Open(_) | crate::fold::Marker::Close => {
                saw_fold = true;
                continue;
            }
            crate::fold::Marker::Line(_) => {}
        }
        let line = strip_terminal_escapes(raw);
        if line.trim_start().starts_with("chat    ") || line.trim_start().starts_with("chat ") {
            saw_chat_line = true;
        }
        if line.trim_start().starts_with("chat    final answer")
            || line.trim_start().starts_with("chat final answer")
        {
            collecting_final = true;
            saw_final = true;
            continue;
        }
        if collecting_final {
            final_answer.push(line);
        } else {
            fallback.push(line);
        }
    }

    let fallback = fallback
        .iter()
        .find(|line| line.trim_start().starts_with("chat error:"))
        .cloned()
        .unwrap_or_else(|| {
            if saw_fold {
                if saw_chat_line {
                    String::new()
                } else {
                    fallback.join("\n")
                }
            } else {
                fallback.join("\n")
            }
        });
    let output = if saw_final {
        final_answer.join("\n")
    } else {
        fallback
    };
    clean_chat_reply(&output)
}

/// Keep the foldable work sections from a streamed chat response.
///
/// The final answer is deliberately excluded: it is stored as the model turn
/// itself. The returned text retains the shared fold markers and all lines
/// inside them, allowing both frontends to render the work after the child has
/// exited and the answer has been delivered.
#[must_use]
pub fn extract_chat_trace(text: &str) -> String {
    let mut depth = 0usize;
    let mut trace = Vec::new();
    for raw in text.lines() {
        match crate::fold::classify(raw) {
            crate::fold::Marker::Open(_) => {
                depth += 1;
                trace.push(raw.to_string());
            }
            crate::fold::Marker::Close => {
                if depth > 0 {
                    trace.push(raw.to_string());
                    depth -= 1;
                }
            }
            crate::fold::Marker::Line(_) if depth > 0 => trace.push(raw.to_string()),
            crate::fold::Marker::Line(_) => {}
        }
    }
    trace.join("\n")
}

/// Strip terminal escapes — CSI colours, OSC hyperlinks, carriage returns —
/// leaving the text itself exactly as it was, indentation included.
///
/// The primitive under [`clean_chat_reply`], separate because a streamed line is
/// not a chat reply: trimming it would flatten the leading spaces that give a
/// run's trace its shape. Reach for this wherever child output is shown by
/// something that does not render escapes at all, such as the visual editor.
#[must_use]
pub fn strip_terminal_escapes(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if matches!(character, '\u{1b}' | '\u{241b}') {
            match chars.next() {
                Some('[') => {
                    for sequence in chars.by_ref() {
                        if ('@'..='~').contains(&sequence) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let mut previous_escape = false;
                    for sequence in chars.by_ref() {
                        if sequence == '\u{7}' {
                            break;
                        }
                        if previous_escape && sequence == '\\' {
                            break;
                        }
                        previous_escape = sequence == '\u{1b}' || sequence == '\u{241b}';
                    }
                }
                Some(_) | None => {}
            }
            continue;
        }
        if character == '\r' {
            continue;
        }
        output.push(character);
    }
    output
}

fn render_run(snapshot: &RunSnapshot<'_>, history: &[(String, String)], question: &str) -> String {
    let snapshot_text = render_run_snapshot(snapshot);
    let history = if history.is_empty() {
        "(first question)".to_string()
    } else {
        let mut bytes: usize = 0;
        let mut start = history.len();
        for (index, (user, assistant)) in history.iter().enumerate().rev() {
            let turn_bytes = user
                .len()
                .saturating_add(assistant.len())
                .saturating_add(32);
            if start < history.len() && bytes.saturating_add(turn_bytes) > MAX_CHAT_HISTORY_BYTES {
                break;
            }
            start = index;
            bytes = bytes.saturating_add(turn_bytes);
            if bytes >= MAX_CHAT_HISTORY_BYTES {
                break;
            }
        }
        let mut rendered = history[start..]
            .iter()
            .map(|(user, assistant)| {
                let user = bounded_tail(user, MAX_CHAT_HISTORY_BYTES / 2, "[...]\n");
                let assistant = bounded_tail(assistant, MAX_CHAT_HISTORY_BYTES / 2, "[...]\n");
                format!(
                    "USER:\n{user}\n\nASSISTANT:\n{}",
                    if assistant.is_empty() {
                        "(turn still running)"
                    } else {
                        assistant.as_ref()
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if start > 0 {
            rendered.insert_str(0, "[... earlier run-chat turns omitted ...]\n\n");
        }
        bounded_tail(
            &rendered,
            MAX_CHAT_HISTORY_BYTES,
            "[... earlier run-chat text omitted ...]\n",
        )
        .into_owned()
    };
    format!(
        "KAOS_RUN_CHAT_DATA\nSNAPSHOT_BYTES={}\n{}\nHISTORY_BYTES={}\n{}\nQUESTION_BYTES={}\n{}\n",
        snapshot_text.len(),
        snapshot_text,
        history.len(),
        history,
        question.len(),
        question
    )
}

fn bounded_tail<'a>(text: &'a str, max_bytes: usize, marker: &str) -> Cow<'a, str> {
    if text.len() <= max_bytes {
        return Cow::Borrowed(text);
    }
    let content_bytes = max_bytes.saturating_sub(marker.len());
    let mut start = text.len().saturating_sub(content_bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    Cow::Owned(format!("{marker}{}", &text[start..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_turn_never_becomes_part_of_the_next_prompt() {
        // The bug this closes: a provider failure was stored as though the
        // model had said it, so every retry carried all previous errors into
        // the prompt — a context that filled with "chat error: oracle failure"
        // and crowded out the actual conversation.
        assert_eq!(
            model_turn_for_history("chat error: oracle failure: ollama http: timed out"),
            None
        );
        assert_eq!(model_turn_for_history("oracle failure: no answer"), None);
        assert_eq!(model_turn_for_history("   "), None);
        // The host's own placeholder for a turn that produced nothing.
        assert_eq!(model_turn_for_history(EMPTY_TURN_NOTICE), None);
        // An ordinary answer is untouched.
        assert_eq!(
            model_turn_for_history("The parser reads the header first."),
            Some("The parser reads the header first.".to_string())
        );
    }

    #[test]
    fn a_stopped_turn_carries_what_the_model_wrote_without_the_hosts_mark() {
        // Stopping keeps the partial answer on screen WITH a mark saying it was
        // stopped. The mark is the host talking, so it must not travel into the
        // next prompt — but the words the model actually wrote should.
        let stored = format!("It parses the header first.\n\n{STOPPED_MARK} mid-answer");
        assert_eq!(
            model_turn_for_history(&stored),
            Some("It parses the header first.".to_string())
        );
        // Stopped before it said anything: nothing to carry.
        assert_eq!(
            model_turn_for_history(&format!("{STOPPED_MARK} before the model answered")),
            None
        );
    }

    /// Every Rebis example in the authoring reference must actually run.
    ///
    /// This file is checked as documentation, so a broken example is caught
    /// before it becomes a misleading authoring reference. Parsing is not
    /// enough to catch that: an undefined macro parses fine and only fails when
    /// the program is run.
    #[test]
    fn every_example_in_the_reference_resolves_and_runs() {
        use std::cell::RefCell;

        struct Any(RefCell<usize>);
        impl rebis_lang::Oracle for Any {
            fn fire(&self, _prompt: &str) -> Option<String> {
                *self.0.borrow_mut() += 1;
                // `1` so a `%` gate takes a branch instead of diagnosing.
                Some("1".to_string())
            }
        }

        // ```rebis fences only: ```text blocks are notation, not programs.
        let mut examples: Vec<(usize, String)> = Vec::new();
        let mut current: Option<(usize, Vec<&str>)> = None;
        for (number, line) in REBIS_AUTHORING_GUIDE.lines().enumerate() {
            match current.as_mut() {
                None if line.trim() == "```rebis" => current = Some((number + 2, Vec::new())),
                None => {}
                Some(_) if line.trim() == "```" => {
                    let (start, body) = current.take().expect("open fence");
                    examples.push((start, body.join("\n")));
                }
                Some((_, body)) => body.push(line),
            }
        }
        assert!(
            examples.len() >= 7,
            "the reference should carry its worked examples; found {}",
            examples.len()
        );

        for (line, source) in examples {
            let expression = rebis_lang::parse(&source)
                .unwrap_or_else(|error| panic!("line {line}: {error}\n{source}"));
            let oracle = Any(RefCell::new(0));
            let mut record = rebis_lang::Record::from_texts::<&str>(&[]);
            let result = rebis_lang::orchestrate(&expression, &mut record, &oracle);
            assert!(
                result.diagnostics.is_empty(),
                "line {line}: {:?}\n{source}",
                result.diagnostics
            );
        }
    }

    #[test]
    fn ordinary_chat_keeps_the_reference_out_of_user_data() {
        let prompt = render_chat_prompt("", "help me write a sigil");
        assert!(!prompt.contains("REBIS AUTHORING AND TESTING REFERENCE"));
        assert!(prompt.contains("KAOS_CHAT_DATA"));
        assert!(prompt.contains("MODE=direct"));
        assert!(prompt.contains("help me write a sigil"));
    }

    #[test]
    fn every_chat_turn_has_a_parseable_rebis_prompt_source() {
        let source = chat_program_source(&render_chat_prompt("", "say (hello) and $ safely"));
        let expression = rebis_lang::parse(&source).expect("chat source should parse");
        assert!(matches!(expression, rebis_lang::Expr::Prompt(_)));
        assert!(source.contains("KAOS_CHAT_DATA"));
        assert!(source.contains("say (hello) and $ safely"));
    }

    #[test]
    fn rebis_agent_context_is_complete_data_with_explicit_boundaries() {
        let rendered = render_rebis_agent_context(RebisAgentContext {
            source: "(-> \"inspect\" \"write\")",
            record: "seed answer",
            scope: "s2:b1",
            model: "ollama:qwen3.6:35b",
            workspace: "/workspace",
            directive: Some("check the regression"),
            attachments: &[("diagram.pdf", "application/pdf")],
            node_prompt: "write\nINPUT:\nRESULT 1:\nanswer",
        });
        assert!(rendered.starts_with("KAOS_REBIS_AGENT_CONTEXT\nVERSION=1\n"));
        assert!(
            rendered.contains("PROGRAM_BYTES=")
                && rendered.contains("(-> \"inspect\" \"write\")\n")
        );
        assert!(rendered.contains("INITIAL_RECORD_BYTES=11\nseed answer\n"));
        assert!(
            rendered.contains("SUPERVISOR_DIRECTIVE_BYTES=")
                && rendered.contains("check the regression\n")
        );
        assert!(
            rendered.contains("ATTACHMENT_0_MEDIA_TYPE_BYTES=")
                && rendered.contains("application/pdf\n")
        );
        assert!(rendered.contains("NODE_PROMPT_BYTES="));
        assert!(rendered.contains("INPUT:\nRESULT 1:\nanswer"));
    }

    #[test]
    fn ordinary_chat_uses_recent_context_without_retaining_an_unbounded_prompt() {
        let history = format!(
            "oldest sentinel\n{}\nnewest sentinel",
            "history ".repeat(MAX_CHAT_HISTORY_BYTES)
        );
        let prompt = render_chat_prompt(&history, "continue");

        assert!(!prompt.contains("oldest sentinel"));
        assert!(prompt.contains("newest sentinel"));
        assert!(prompt.contains("earlier conversation omitted"));
        assert!(prompt.len() < MAX_CHAT_HISTORY_BYTES + 2_000);
    }

    #[test]
    fn system_contract_is_not_augmented_by_chat_context() {
        let system = DEFAULT_CONTEXT.augment_system("base contract");
        assert_eq!(system, "base contract");
        assert_eq!(
            DEFAULT_CONTEXT.augment_system_with_library("base contract"),
            "base contract"
        );
    }

    #[test]
    fn chat_reply_cleaner_removes_terminal_sequences_and_overwrites() {
        assert_eq!(
            clean_chat_reply("\u{1b}[31mred\u{1b}[0m\r\nnext\u{1b}]8;;url\u{7}link\u{1b}]8;;\u{7}"),
            "red\nnextlink"
        );
        assert_eq!(clean_chat_reply("␛[2mvisible␛[0m"), "visible");
    }

    #[test]
    fn streamed_chat_reply_excludes_folded_work() {
        let stream = concat!(
            "\u{1e}FOLD_OPEN\u{1f}model turn 1 · working\n",
            "chat    generating turn 1\n",
            "\u{1e}FOLD_CLOSE\n",
            "chat    final answer\n",
            "Here is the answer.\n",
            "\n",
            "Second paragraph.\n",
        );
        assert_eq!(
            extract_chat_reply(stream),
            "Here is the answer.\n\nSecond paragraph."
        );
    }

    #[test]
    fn streamed_chat_trace_keeps_work_after_the_answer_is_delivered() {
        let stream = concat!(
            "started     chat\n",
            "\u{1e}FOLD_OPEN\u{1f}model turn 1 · complete response\n",
            "model    generated a Rebis program\n",
            "\u{1e}FOLD_CLOSE\n",
            "chat    final answer\n",
            "Here is the summary.\n",
            "complete     process exited 0\n",
        );
        assert_eq!(
            extract_chat_trace(stream),
            concat!(
                "\u{1e}FOLD_OPEN\u{1f}model turn 1 · complete response\n",
                "model    generated a Rebis program\n",
                "\u{1e}FOLD_CLOSE"
            )
        );
    }

    #[test]
    fn final_only_chat_reply_stays_compatible() {
        assert_eq!(
            extract_chat_reply("plain answer\nnext line\n"),
            "plain answer\nnext line"
        );
    }

    #[test]
    fn folded_non_chat_output_is_not_dropped_from_the_session() {
        let stream = concat!(
            "\u{1e}FOLD_OPEN\u{1f}inner working\n",
            "model    editing the requested file\n",
            "\u{1e}FOLD_CLOSE\n",
        );
        assert_eq!(
            extract_chat_reply(stream),
            "model    editing the requested file"
        );
    }

    #[test]
    fn interrupted_stream_does_not_become_a_model_answer() {
        let stream = concat!(
            "\u{1e}FOLD_OPEN\u{1f}model turn 1 · working\n",
            "chat    model turn 1 · working\n",
            "\u{1e}FOLD_CLOSE\n",
        );
        assert!(extract_chat_reply(stream).is_empty());
        assert_eq!(
            extract_chat_reply(
                "\u{1e}FOLD_OPEN\u{1f}work\nchat error: offline\n\u{1e}FOLD_CLOSE\n"
            ),
            "chat error: offline"
        );
    }

    #[test]
    fn stripping_escapes_keeps_the_text_and_its_indentation() {
        // A streamed trace line carries its shape in its leading spaces, so the
        // primitive must not trim — only the chat cleaner does.
        let painted = "     \u{1b}[38;2;16;16;16m\u{2192} exit 0\u{1b}[0m";
        assert_eq!(strip_terminal_escapes(painted), "     \u{2192} exit 0");
        assert_eq!(
            clean_chat_reply(painted),
            "\u{2192} exit 0",
            "the chat cleaner still trims, as durable history wants"
        );

        // The exact shape the visual editor was showing: a true-colour run with
        // a background, both codes leaking through as their own digits.
        let leaked = "\u{1b}[48;2;250;250;250;38;2;74;124;240m- 1964-1973\u{1b}[0m";
        assert_eq!(strip_terminal_escapes(leaked), "- 1964-1973");

        // Text with no escapes is returned untouched, whitespace included.
        assert_eq!(strip_terminal_escapes("  plain  "), "  plain  ");
    }

    #[test]
    fn run_chat_keeps_every_output_line_and_previous_turns() {
        let output = vec!["first".to_string(), "second".to_string()];
        let snapshot = RunSnapshot {
            id: 7,
            state: "RUNNING",
            paused: false,
            pause_reason: "none",
            scope: "program",
            mode: "DIRECT",
            lane: "serial",
            timer: "TIME 1s",
            child: "yes",
            source: "\"inspect\"",
            input: "record",
            output: &output,
        };
        let prompt = render_run_chat(
            &snapshot,
            &[("what happened?".to_string(), "first answer".to_string())],
            "what is next?",
        );
        let visible = render_run_snapshot(&snapshot);
        assert!(visible.contains("SOURCE\n\"inspect\""));
        assert!(visible.contains("RECORD / INPUT\nrecord"));
        assert!(prompt.contains("first\nsecond"));
        assert!(prompt.contains("what happened?"));
        assert!(prompt.contains("what is next?"));
    }

    /// The stance decides the envelope, and one call site is all a frontend
    /// needs — the terminal app, the visual editor and `kaos chat` each pass a
    /// boolean here rather than each writing its own branch.
    #[test]
    fn the_stance_selects_the_envelope_and_neither_loses_the_conversation() {
        let history = "USER: earlier\nASSISTANT: noted";
        let direct = DEFAULT_CONTEXT.render_turn(false, history, "rename the flag");
        let chaos = DEFAULT_CONTEXT.render_turn(true, history, "rename the flag");

        assert_eq!(
            direct,
            DEFAULT_CONTEXT.render_chat(history, "rename the flag")
        );
        assert_eq!(
            chaos,
            DEFAULT_CONTEXT.render_chaos_chat(history, "rename the flag")
        );
        // A composed turn still answers a conversation: the turn before it is
        // often what says which file the program is about.
        for rendered in [&direct, &chaos] {
            assert!(rendered.contains("earlier"), "history was dropped");
            assert!(rendered.contains("rename the flag"));
        }
        assert!(chaos.contains("MODE=chaos"));
        assert!(direct.contains("MODE=direct"));
    }

    #[test]
    fn chaos_turn_is_data_for_the_rebis_composer_not_a_hidden_contract() {
        let chaos = DEFAULT_CONTEXT.render_chaos_chat("", "count the macros");
        let direct = DEFAULT_CONTEXT.render_chat("", "count the macros");
        assert!(chaos.contains("MODE=chaos"));
        assert!(!chaos.contains(crate::chaos::COMPOSITION_REQUEST));
        assert!(!direct.contains(crate::chaos::COMPOSITION_REQUEST));
        assert!(!chaos.contains("```rebis"));
    }

    #[test]
    fn chaos_system_context_does_not_inject_a_composer_contract() {
        let system = DEFAULT_CONTEXT.augment_system_for_chaos("BASE CONTRACT");
        assert_eq!(system, "BASE CONTRACT");
        assert!(!system.contains(REBIS_AUTHORING_GUIDE));
        assert!(!system.contains(crate::chaos::COMPOSITION_REQUEST));
    }
}
