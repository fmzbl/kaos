//! The Familiar — a general tool-using chat agent over a caller-supplied toolset.
//!
//! The [`conductor`](crate::conductor) is an agent bound to ONE world: a
//! filesystem workspace, with `read_file`/`edit_file`/`bash` baked in. That is
//! the right shape for coding, and the wrong shape for everything else. A
//! familiar is the same loop with the world pulled out behind a trait: kaos owns
//! *when* to call a tool (the `<act>` protocol, the banishment law, the twin
//! ladders of [`charge`](crate::charge)); the host owns *what* the tools are and
//! *what they do*. A witch's familiar knows the house it serves — kaos does not
//! need to.
//!
//! ```text
//!   loop:
//!     reply = model(system + transcript)     // the familiar thinks
//!     act   = parse one <act> from reply      // it chooses a tool
//!     if finish: return its message           // it answers
//!     obs   = toolset.invoke(act)             // the host answers the tool
//!     transcript += act + obs                 // it sees the result, continues
//! ```
//!
//! The model is behind the [`Chat`] seam, so a familiar
//! runs on claude, a local ollama model, or a scripted stub, unchanged.

use std::collections::BTreeMap;

use crate::conductor::Chat;

/// The world a familiar acts in. The host implements this to expose whatever set
/// of tools it wants the agent to have — read a record, search a store, call an
/// API. kaos never sees inside; it only relays the model's chosen tool and hands
/// back the observation the host returns.
pub trait Toolset {
    /// A short catalogue of the available tools, rendered into the system prompt.
    /// One line per tool is ideal, e.g. `- search: args query — find records`.
    /// The `finish` tool is added by the loop and must not be listed here.
    fn catalogue(&self) -> String;

    /// Run one tool by name with its parsed string arguments, returning the
    /// observation the model will read next. An unknown tool or bad argument
    /// should return a short `error: …` string (which the loop treats as a
    /// negative observation) rather than panicking — a familiar recovers.
    fn invoke(&self, tool: &str, args: &BTreeMap<String, String>) -> String;
}

/// One executed step of a conversation, for the caller's trace.
#[derive(Clone, Debug)]
pub struct Turn {
    /// The model's visible reasoning around the action (text outside `<act>`).
    pub thought: String,
    /// The tool it chose.
    pub tool: String,
    /// The arguments it passed.
    pub args: BTreeMap<String, String>,
    /// What the toolset returned (or, for `finish`, the final answer).
    pub observation: String,
}

/// The record of a familiar's session.
#[derive(Clone, Debug)]
pub struct Conversation {
    pub steps: Vec<Turn>,
    /// The final answer to relay to the user. Empty only on `error`.
    pub answer: String,
    pub error: Option<String>,
}

/// The most steps a familiar may take before it is made to answer from what it
/// has. A chat agent gathers a few facts and speaks; it does not spelunk.
pub const DEFAULT_MAX_STEPS: usize = 6;

/// Run a familiar over `question`, letting it call `tools` until it finishes.
///
/// `role` is the host's one-paragraph charge — who the agent is and what it is
/// for; it heads the system prompt. On a clean finish the `finish` message is
/// the answer; if the step budget is spent first, one last tool-less turn forces
/// a direct answer from the gathered context (a familiar always speaks).
pub fn converse(
    role: &str,
    question: &str,
    tools: &dyn Toolset,
    chat: &dyn Chat,
    max_steps: usize,
) -> Conversation {
    let system = system_prompt(role, &tools.catalogue());
    let mut steps: Vec<Turn> = Vec::new();
    // (acted, observation) per turn, rendered through the twin ladders each pass.
    let mut turns: Vec<(String, String)> = Vec::new();
    let mut nudges = 0usize;

    while steps.len() < max_steps {
        let transcript = render_transcript(question, &turns);
        let reply = match chat.respond(&system, &transcript) {
            Ok(r) => r,
            Err(e) => {
                return Conversation {
                    steps,
                    answer: String::new(),
                    error: Some(e),
                }
            }
        };
        if crate::config::enabled("KAOS_DEBUG") {
            eprintln!("\n=== FAMILIAR REPLY ===\n{reply}\n=== END ===");
        }

        let Some((tool, args)) = parse_first_act(&reply) else {
            // Banish the malformed reply; keep only a nudge. Three in a row and
            // the mind is not speaking the protocol — end rather than spend.
            nudges += 1;
            if nudges >= 3 {
                return force_answer(role, question, &turns, chat, steps);
            }
            turns.push((
                "(your previous reply held no <act> block and was banished)".to_string(),
                "reply with exactly one <act tool=\"…\">…</act> block; to answer, use the finish tool."
                    .to_string(),
            ));
            continue;
        };
        nudges = 0;
        let thought = thought_of(&reply);

        if is_finish(&tool) {
            let answer = first_of(&args, &["message", "answer", "summary", "text"]);
            steps.push(Turn {
                thought,
                tool,
                args,
                observation: answer.clone(),
            });
            return Conversation {
                steps,
                answer,
                error: None,
            };
        }

        let observation = tools.invoke(&tool, &args);
        turns.push((act_block_of(&reply), observation.clone()));
        steps.push(Turn {
            thought,
            tool,
            args,
            observation,
        });
    }

    // Budget spent without a finish: make it answer now, from what it gathered.
    force_answer(role, question, &turns, chat, steps)
}

/// One last tool-less turn: answer the question directly from the gathered
/// context. Used when the model stops speaking the protocol or runs out of
/// steps — the familiar still owes the user a reply.
fn force_answer(
    role: &str,
    question: &str,
    turns: &[(String, String)],
    chat: &dyn Chat,
    steps: Vec<Turn>,
) -> Conversation {
    let system = format!(
        "{role}\n\nAnswer the user's question now, in plain prose. Use the notes below for \
         what they established, and your own knowledge for what the notes cannot know — \
         never claim the notes contain something they do not. Do not call tools. Be direct \
         and concrete."
    );
    let transcript = render_transcript(question, turns);
    match chat.respond(&system, &transcript) {
        Ok(answer) => Conversation {
            steps,
            answer: strip_acts(&answer).trim().to_string(),
            error: None,
        },
        Err(e) => Conversation {
            steps,
            answer: String::new(),
            error: Some(e),
        },
    }
}

/// The system prompt: the host's role, the `<act>` liturgy, and the catalogue.
fn system_prompt(role: &str, catalogue: &str) -> String {
    format!(
        "{role}\n\n\
         You work by calling tools to gather facts, then giving a final answer. Each turn, \
         reply with EXACTLY ONE action and nothing else, in this format:\n\
         <act tool=\"TOOL\">\n<arg name=\"KEY\">VALUE</arg>\n</act>\n\n\
         Tools:\n\
         {catalogue}\n\
         - finish: args message (your final answer to the user — call this when you have enough)\n\n\
         Gather a few facts with the tools, then finish. Keep it to a handful of steps. Never \
         invent tool results: any claim drawn from the tools must match what they actually \
         returned. For what the tools cannot know, use your own knowledge plainly — a tool \
         returning nothing is not evidence about the world, only about the tools. When you \
         finish, write the answer for a person, in plain prose — not JSON."
    )
}

/// Render the transcript through the twin ladders of [`charge`](crate::charge):
/// the question is never cut, fresh observations burn bright, the middle decays,
/// and each observation's polarity picks the surviving end. Shared law with the
/// [`conductor`](crate::conductor).
fn render_transcript(question: &str, turns: &[(String, String)]) -> String {
    let n = 1 + turns.len();
    let mut out = format!("QUESTION: {question}\n\nBegin. Reply with one <act> block.");
    for (i, (acted, observation)) in turns.iter().enumerate() {
        let limit = crate::charge::budget_kinded(i + 1, n, observation);
        let negative = crate::charge::is_negative(observation);
        let obs = crate::charge::cut(observation, limit, negative);
        out.push_str(&format!("\n\nAssistant: {acted}\n\nOBSERVATION:\n{obs}"));
    }
    out
}

fn is_finish(tool: &str) -> bool {
    matches!(tool, "finish" | "done" | "stop" | "answer")
}

fn first_of(args: &BTreeMap<String, String>, keys: &[&str]) -> String {
    for k in keys {
        if let Some(v) = args.get(*k) {
            return v.clone();
        }
    }
    String::new()
}

/// Parse the FIRST `<act tool="…">…</act>` block: its tool name and args.
fn parse_first_act(text: &str) -> Option<(String, BTreeMap<String, String>)> {
    let start = text.find("<act")?;
    let open_end = text[start..].find('>')? + start;
    let header = &text[start..open_end];
    let tool = attr(header, "tool")?;
    let block_end = text[open_end..]
        .find("</act>")
        .map(|i| i + open_end)
        .unwrap_or(text.len());
    let body = &text[open_end + 1..block_end];
    Some((tool, parse_args(body)))
}

fn attr(header: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let a = header.find(&pat)? + pat.len();
    let b = header[a..].find('"')? + a;
    Some(header[a..b].to_string())
}

fn parse_args(body: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut rest = body;
    while let Some(a) = rest.find("<arg name=\"") {
        let name_start = a + "<arg name=\"".len();
        let Some(nq) = rest[name_start..].find('"') else {
            break;
        };
        let name = rest[name_start..name_start + nq].to_string();
        let Some(gt) = rest[name_start + nq..].find('>') else {
            break;
        };
        let val_start = name_start + nq + gt + 1;
        let Some(close) = rest[val_start..].find("</arg>") else {
            break;
        };
        let val = rest[val_start..val_start + close]
            .trim_matches('\n')
            .to_string();
        map.insert(name, val);
        rest = &rest[val_start + close + "</arg>".len()..];
    }
    map
}

/// The reply text outside the `<act>` block — the model's visible reasoning,
/// trimmed and bounded.
fn thought_of(reply: &str) -> String {
    let outside = match reply.find("<act") {
        Some(start) => {
            let after = reply[start..]
                .find("</act>")
                .map(|i| &reply[start + i + "</act>".len()..])
                .unwrap_or("");
            format!("{}{}", &reply[..start].trim_end(), after)
        }
        None => reply.to_string(),
    };
    let t = outside.trim();
    t.chars().take(600).collect()
}

/// The `<act>…</act>` span, first block through last, verbatim.
fn act_block_of(reply: &str) -> String {
    match (reply.find("<act"), reply.rfind("</act>")) {
        (Some(a), Some(b)) if b > a => reply[a..b + "</act>".len()].to_string(),
        _ => reply.trim().to_string(),
    }
}

/// Remove any stray `<act>` blocks from a final prose answer.
fn strip_acts(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find("<act") {
        out.push_str(&rest[..a]);
        match rest[a..].find("</act>") {
            Some(i) => rest = &rest[a + i + "</act>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conductor::ScriptedChat;

    struct Echo;
    impl Toolset for Echo {
        fn catalogue(&self) -> String {
            "- lookup: args key — return a canned fact".to_string()
        }
        fn invoke(&self, tool: &str, args: &BTreeMap<String, String>) -> String {
            match tool {
                "lookup" => format!("fact about {}: it is blue", first_of(args, &["key"])),
                other => format!("error: unknown tool {other}"),
            }
        }
    }

    #[test]
    fn gathers_then_finishes() {
        let chat = ScriptedChat::new(vec![
            "I will look it up.\n<act tool=\"lookup\">\n<arg name=\"key\">sky</arg>\n</act>",
            "<act tool=\"finish\">\n<arg name=\"message\">The sky is blue.</arg>\n</act>",
        ]);
        let c = converse(
            "You are a test familiar.",
            "what colour is the sky?",
            &Echo,
            &chat,
            6,
        );
        assert!(c.error.is_none());
        assert_eq!(c.answer, "The sky is blue.");
        assert_eq!(c.steps.len(), 2);
        assert_eq!(c.steps[0].tool, "lookup");
        assert!(c.steps[0].observation.contains("blue"));
    }

    #[test]
    fn parses_tool_and_args() {
        let (tool, args) = parse_first_act(
            "pre <act tool=\"search\">\n<arg name=\"query\">rust</arg>\n</act> post",
        )
        .unwrap();
        assert_eq!(tool, "search");
        assert_eq!(args.get("query").unwrap(), "rust");
    }
}
