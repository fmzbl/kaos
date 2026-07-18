//! The Open Hand — native tool-calling, the same rites in the mind's own tongue.
//!
//! The k2.7 verdict (docs/EDGE.md) was structural: models RL-trained for
//! native tool-calling pay a heavy tax when driven through a text-parsed
//! protocol. The Open Hand removes the tax without surrendering the doctrine:
//! the SAME five tools, the SAME executor (with its lint veto and workspace
//! isolation), the SAME Twin Ladders of charge over the history — but spoken
//! as structured `tool_calls`, the dialect the mind was trained in.
//!
//! Backend parity is a hard requirement: OpenAI-compatible hosts (OpenRouter,
//! OpenAI) get `tools` on `/v1/chat/completions`; ollama gets `tools` on
//! `/api/chat`. One message shape, one budget law, everywhere.

use crate::conductor::Tool;

/// One turn of a native conversation. `content` is the assistant's narration
/// or a tool's observation; `calls` is non-empty only for assistant turns
/// that requested tools.
#[derive(Clone, Debug)]
pub struct Msg {
    pub role: Role,
    pub content: String,
    /// (call_id, tool) pairs the assistant requested this turn.
    pub calls: Vec<(String, Tool)>,
    /// For Role::Tool: the id of the call this observation answers.
    pub call_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

impl Msg {
    pub fn user(content: impl Into<String>) -> Msg {
        Msg {
            role: Role::User,
            content: content.into(),
            calls: Vec::new(),
            call_id: String::new(),
        }
    }
    pub fn assistant(content: impl Into<String>, calls: Vec<(String, Tool)>) -> Msg {
        Msg {
            role: Role::Assistant,
            content: content.into(),
            calls,
            call_id: String::new(),
        }
    }
    pub fn tool(call_id: impl Into<String>, observation: impl Into<String>) -> Msg {
        Msg {
            role: Role::Tool,
            content: observation.into(),
            calls: Vec::new(),
            call_id: call_id.into(),
        }
    }
}

/// The five tools as JSON Schema, in the OpenAI `tools` shape both OpenRouter
/// and ollama accept. One definition serves every backend — parity by
/// construction.
pub fn tool_schemas() -> serde_json::Value {
    let f = |name: &str, desc: &str, props: serde_json::Value, req: &[&str]| {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": name,
                "description": desc,
                "parameters": {
                    "type": "object",
                    "properties": props,
                    "required": req,
                }
            }
        })
    };
    serde_json::json!([
        f(
            "read_file",
            "Read a file (relative path inside the project).",
            serde_json::json!({ "path": {"type": "string"} }),
            &["path"]
        ),
        f(
            "write_file",
            "Create or overwrite a file with the given complete contents.",
            serde_json::json!({ "path": {"type": "string"}, "contents": {"type": "string"} }),
            &["path", "contents"]
        ),
        f(
            "edit_file",
            "Replace the first exact occurrence of `find` with `replace` in the file.",
            serde_json::json!({ "path": {"type": "string"}, "find": {"type": "string"}, "replace": {"type": "string"} }),
            &["path", "find", "replace"]
        ),
        f(
            "bash",
            "Run a shell command in the project root (ls, grep, run tests).",
            serde_json::json!({ "cmd": {"type": "string"} }),
            &["cmd"]
        ),
        f(
            "finish",
            "Call when the task is done AND verified; `message` summarizes what changed.",
            serde_json::json!({ "message": {"type": "string"} }),
            &["message"]
        ),
    ])
}

/// Parse one tool call's (name, JSON arguments) into a [`Tool`]. Unknown names
/// and malformed arguments return None — the loop banishes those calls.
pub fn parse_call(name: &str, args: &serde_json::Value) -> Option<Tool> {
    let s = |k: &str| args.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match name {
        "read_file" => Some(Tool::ReadFile { path: s("path")? }),
        "write_file" => Some(Tool::WriteFile {
            path: s("path")?,
            contents: s("contents")?,
        }),
        "edit_file" => Some(Tool::EditFile {
            path: s("path")?,
            find: s("find")?,
            replace: s("replace")?,
        }),
        "bash" => Some(Tool::Bash { cmd: s("cmd")? }),
        "finish" => Some(Tool::Finish {
            message: s("message").unwrap_or_default(),
        }),
        _ => None,
    }
}

/// Render the history into OpenAI-shape `messages`, the Twin Ladders applied:
/// message 0 (the intent) is never compressed; every other message is cut to
/// its fib budget with polarity picking the surviving end. Both hosted and
/// ollama chat endpoints accept this exact shape.
pub fn render_messages(system: &str, history: &[Msg]) -> serde_json::Value {
    let n = history.len();
    let mut out = vec![serde_json::json!({ "role": "system", "content": system })];
    for (i, m) in history.iter().enumerate() {
        // Position (the twin ladders) OR nature (edits/verdicts), whichever
        // burns brighter — the tunnel's second law.
        let limit = crate::charge::budget_kinded(i, n, &m.content);
        let negative = crate::charge::is_negative(&m.content);
        let content = crate::charge::cut(&m.content, limit, negative);
        match m.role {
            Role::User => out.push(serde_json::json!({ "role": "user", "content": content })),
            Role::Assistant => {
                let mut msg = serde_json::json!({ "role": "assistant", "content": content });
                if !m.calls.is_empty() {
                    msg["tool_calls"] = serde_json::Value::Array(
                        m.calls
                            .iter()
                            .map(|(id, t)| tool_call_json(id, t))
                            .collect(),
                    );
                }
                out.push(msg);
            }
            Role::Tool => out.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": m.call_id,
                "content": content,
            })),
        }
    }
    serde_json::Value::Array(out)
}

/// A [`Tool`] back into the OpenAI tool_call wire shape (for history replay).
fn tool_call_json(id: &str, t: &Tool) -> serde_json::Value {
    let (name, args) = match t {
        Tool::ReadFile { path } => ("read_file", serde_json::json!({ "path": path })),
        Tool::WriteFile { path, contents } => (
            "write_file",
            serde_json::json!({ "path": path, "contents": contents }),
        ),
        Tool::EditFile {
            path,
            find,
            replace,
        } => (
            "edit_file",
            serde_json::json!({ "path": path, "find": find, "replace": replace }),
        ),
        Tool::Bash { cmd } => ("bash", serde_json::json!({ "cmd": cmd })),
        Tool::Finish { message } => ("finish", serde_json::json!({ "message": message })),
    };
    serde_json::json!({
        "id": id,
        "type": "function",
        "function": { "name": name, "arguments": args.to_string() }
    })
}

/// What one native completion returned: narration plus requested calls.
#[derive(Clone, Debug, Default)]
pub struct NativeReply {
    pub content: String,
    pub calls: Vec<(String, Tool)>,
}

/// Parse an OpenAI-shape assistant message (`choices[0].message`) into a
/// [`NativeReply`]. ollama's `/api/chat` response `message` parses with the
/// same function — its tool_calls carry `function.arguments` as an object
/// rather than a string, and both are handled.
pub fn parse_reply(message: &serde_json::Value) -> NativeReply {
    let content = message["content"].as_str().unwrap_or("").to_string();
    let mut calls = Vec::new();
    for (i, c) in message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let name = c["function"]["name"].as_str().unwrap_or("");
        let raw_args = &c["function"]["arguments"];
        let args: serde_json::Value = match raw_args {
            serde_json::Value::String(s) => serde_json::from_str(s).unwrap_or_default(),
            other => other.clone(),
        };
        if let Some(tool) = parse_call(name, &args) {
            let id = c["id"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{i}"));
            calls.push((id, tool));
        }
    }
    NativeReply {
        content: crate::backend::strip_think(&content).trim().to_string(),
        calls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_cover_the_five_tools() {
        let v = tool_schemas();
        let names: Vec<&str> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["read_file", "write_file", "edit_file", "bash", "finish"]
        );
    }

    #[test]
    fn parse_call_round_trips_each_tool() {
        let t = parse_call(
            "edit_file",
            &serde_json::json!({
                "path": "a.py", "find": "x", "replace": "y"
            }),
        )
        .unwrap();
        assert_eq!(
            t,
            Tool::EditFile {
                path: "a.py".into(),
                find: "x".into(),
                replace: "y".into()
            }
        );
        assert!(parse_call("rm_rf", &serde_json::json!({})).is_none());
        assert!(
            parse_call("read_file", &serde_json::json!({})).is_none(),
            "missing args banished"
        );
    }

    #[test]
    fn parse_reply_handles_string_and_object_arguments() {
        // hosted shape: arguments as a JSON STRING
        let hosted = serde_json::json!({
            "content": "fixing now",
            "tool_calls": [{ "id": "c1", "type": "function",
                "function": { "name": "bash", "arguments": "{\"cmd\": \"ls\"}" } }]
        });
        let r = parse_reply(&hosted);
        assert_eq!(r.calls.len(), 1);
        assert_eq!(r.calls[0].1, Tool::Bash { cmd: "ls".into() });
        // ollama shape: arguments as an OBJECT
        let local = serde_json::json!({
            "content": "",
            "tool_calls": [{ "function": { "name": "read_file", "arguments": { "path": "f.txt" } } }]
        });
        let r2 = parse_reply(&local);
        assert_eq!(
            r2.calls[0].1,
            Tool::ReadFile {
                path: "f.txt".into()
            }
        );
        assert_eq!(r2.calls[0].0, "call_0", "missing id gets a synthetic one");
    }

    #[test]
    fn render_applies_the_ladders_and_replays_calls() {
        let mut history = vec![Msg::user("TASK: fix the bug")];
        for i in 0..12 {
            history.push(Msg::assistant(
                "",
                vec![(format!("c{i}"), Tool::Bash { cmd: "ls".into() })],
            ));
            history.push(Msg::tool(format!("c{i}"), "x".repeat(3000)));
        }
        let v = render_messages("sys", &history);
        let arr = v.as_array().unwrap();
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[1]["content"], "TASK: fix the bug"); // the intent, whole
                                                            // a middle TOOL observation is cut to base charge (500) + the banish mark
        let mid = arr
            .iter()
            .filter(|m| m["role"] == "tool")
            .nth(4) // the 5th of 12 observations — deep in the rotting middle
            .unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(
            mid.len() < 700,
            "middle observation must rot: {}",
            mid.len()
        );
        assert!(mid.contains("banished"));
        // the freshest observation keeps its full 3000 chars (ladder anchor)
        let last = arr.last().unwrap()["content"].as_str().unwrap();
        assert!(last.len() >= 3000);
        // assistant turns replay their tool_calls
        assert!(arr[2]["tool_calls"].is_array());
    }
}
