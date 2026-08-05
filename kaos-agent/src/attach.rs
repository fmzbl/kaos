//! Putting a file on a provider's wire.
//!
//! A Rebis program says `(&: "./bug.png")` and the runtime carries the bytes to
//! whichever model is about to fire. Between those two facts sits one question
//! that every provider answers differently — *how does a file become part of a
//! request* — and this module is the only place that knows.
//!
//! It is a module rather than four blocks of JSON inside four request builders
//! because the variation is real but narrow. Three shapes exist:
//!
//! - Anthropic puts typed blocks in the `content` array, and distinguishes a
//!   picture (`image`) from a document (`document`).
//! - OpenAI-compatible hosts put a data URI in an `image_url` block, and take
//!   pictures only.
//! - Ollama keeps `content` as a string and hangs bare base64 off an `images`
//!   field beside it.
//!
//! Everything else about a request — the model, the sampling, the timeout, the
//! error handling — is identical whether a file came along or not, so the
//! request builders keep owning that and ask here only for the user message.
//!
//! **What a provider cannot carry is refused, never dropped.** A file that is
//! silently discarded is worse than an error: the model answers confidently
//! about a picture it never saw, and nothing in the transcript says so. Every
//! wire therefore reports what it will not take, with the reason, and the
//! caller turns that into a diagnostic the run can show.

use rebis_lang::Attachment;

/// The shape a provider family expects a file in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wire {
    /// Typed blocks in `content`: `image` for pictures, `document` for PDFs.
    Anthropic,
    /// A `data:` URI in an `image_url` block. Pictures only.
    OpenAi,
    /// `content` stays a string; base64 hangs off an `images` field.
    Ollama,
    /// No wire encoding at all — the model reads the file itself.
    ///
    /// This is the coding-agent CLI, which has its own filesystem tools, and
    /// the simulated mind, which has nothing to read with. The program already
    /// names the path in its own text (that is what `(&: path)` answers with),
    /// so a CLI agent finds the file the way it finds any other. Nothing here
    /// needs to encode it, and inventing a flag to hand it over would be
    /// worse than letting the agent do what it is for.
    Read,
}

/// Whether a media type is a picture rather than a document.
///
/// The line every wire draws in some form, so it is drawn once here.
fn is_image(media_type: &str) -> bool {
    media_type.starts_with("image/")
}

impl Wire {
    /// What this wire will not take, and why — one message per refused file.
    ///
    /// Empty when everything fits. A caller reports these rather than sending
    /// a request that quietly omits half of what the program attached.
    #[must_use]
    pub fn refusals(self, files: &[Attachment]) -> Vec<String> {
        files
            .iter()
            .filter_map(|file| match self {
                // Both shapes exist here, so nothing is refused.
                Wire::Anthropic | Wire::Read => None,
                Wire::OpenAi if is_image(&file.media_type) => None,
                Wire::OpenAi => Some(format!(
                    "{} is {} — this provider takes images only; \
                     an Anthropic model or the Claude CLI can read it",
                    file.name, file.media_type
                )),
                Wire::Ollama if is_image(&file.media_type) => None,
                Wire::Ollama => Some(format!(
                    "{} is {} — a local vision model takes images only",
                    file.name, file.media_type
                )),
            })
            .collect()
    }

    /// The complete user message carrying `text` and whatever of `files` this
    /// wire can take.
    ///
    /// Files come **before** the text in the block orders that have one. Both
    /// providers document that a model attends better to a question asked after
    /// the material than before it, and it is the same reason framing goes
    /// ahead of a prompt: the thing being reasoned about, then the asking.
    #[cfg(feature = "api")]
    #[must_use]
    pub fn user_message(self, text: &str, files: &[Attachment]) -> serde_json::Value {
        use serde_json::json;
        let carried: Vec<&Attachment> = files
            .iter()
            .filter(|file| match self {
                Wire::Anthropic | Wire::Read => true,
                Wire::OpenAi | Wire::Ollama => is_image(&file.media_type),
            })
            .collect();
        if carried.is_empty() || self == Wire::Read {
            return json!({ "role": "user", "content": text });
        }
        match self {
            Wire::Anthropic => {
                let mut content: Vec<serde_json::Value> = carried
                    .iter()
                    .map(|file| {
                        json!({
                            // A picture and a document are different block
                            // kinds to this API, and sending one as the other
                            // is a 400 rather than a degraded answer.
                            "type": if is_image(&file.media_type) { "image" } else { "document" },
                            "source": {
                                "type": "base64",
                                "media_type": file.media_type,
                                "data": encode(&file.bytes),
                            },
                        })
                    })
                    .collect();
                content.push(json!({ "type": "text", "text": text }));
                json!({ "role": "user", "content": content })
            }
            Wire::OpenAi => {
                let mut content: Vec<serde_json::Value> = carried
                    .iter()
                    .map(|file| {
                        json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!(
                                    "data:{};base64,{}",
                                    file.media_type,
                                    encode(&file.bytes)
                                ),
                            },
                        })
                    })
                    .collect();
                content.push(json!({ "type": "text", "text": text }));
                json!({ "role": "user", "content": content })
            }
            Wire::Ollama => json!({
                "role": "user",
                "content": text,
                // Bare base64, no data URI, and beside the content rather than
                // inside it — this API kept the message a string and added a
                // field, where the others made the message a list.
                "images": carried.iter().map(|file| encode(&file.bytes)).collect::<Vec<_>>(),
            }),
            Wire::Read => unreachable!("handled above"),
        }
    }

    /// Bare base64 for an API that hangs images off a FIELD rather than
    /// putting them in the message.
    ///
    /// Ollama's completion endpoint keeps the prompt a flat string and takes
    /// an `images` array beside it, so there is no message object to build.
    /// Same knowledge, different shape — and it lives here for the same reason
    /// the rest does.
    #[cfg(feature = "api")]
    #[must_use]
    pub fn images(self, files: &[Attachment]) -> Vec<String> {
        files
            .iter()
            .filter(|file| is_image(&file.media_type))
            .filter(|_| matches!(self, Wire::Ollama | Wire::OpenAi | Wire::Anthropic))
            .map(|file| encode(&file.bytes))
            .collect()
    }

    /// The wire a provider kind speaks.
    #[must_use]
    pub fn of(kind: crate::provider::Kind) -> Self {
        use crate::provider::Kind;
        match kind {
            Kind::ClaudeApi => Wire::Anthropic,
            Kind::OpenAi | Kind::OpenRouter => Wire::OpenAi,
            Kind::Ollama => Wire::Ollama,
            // A coding agent reads the path the program already named; a
            // simulated mind reads nothing at all.
            Kind::ClaudeCli | Kind::Simulated => Wire::Read,
        }
    }
}

/// Base64, the one encoding all three wires agree on.
#[cfg(feature = "api")]
fn encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Attachment {
        Attachment {
            media_type: "image/png".to_string(),
            name: "bug.png".to_string(),
            bytes: vec![0x89, b'P', b'N', b'G'],
        }
    }

    fn document() -> Attachment {
        Attachment {
            media_type: "application/pdf".to_string(),
            name: "spec.pdf".to_string(),
            bytes: b"%PDF-1.7".to_vec(),
        }
    }

    #[test]
    fn a_wire_refuses_what_it_cannot_carry_rather_than_dropping_it() {
        // The whole reason refusals exist. A silently omitted file means the
        // model answers confidently about a picture it never saw, and nothing
        // in the transcript says so.
        assert!(Wire::Anthropic
            .refusals(&[picture(), document()])
            .is_empty());
        assert!(Wire::Read.refusals(&[picture(), document()]).is_empty());
        assert!(Wire::OpenAi.refusals(&[picture()]).is_empty());

        let refused = Wire::OpenAi.refusals(&[document()]);
        assert_eq!(refused.len(), 1);
        assert!(refused[0].contains("spec.pdf"), "{refused:?}");
        // The refusal says where it WOULD work, because a person reading it is
        // trying to get their document read, not to be told no.
        assert!(refused[0].contains("Anthropic"), "{refused:?}");

        let refused = Wire::Ollama.refusals(&[document()]);
        assert_eq!(refused.len(), 1);
        assert!(refused[0].contains("images only"), "{refused:?}");
    }

    #[cfg(feature = "api")]
    #[test]
    fn each_wire_builds_the_shape_its_provider_documents() {
        let files = [picture(), document()];

        // Anthropic: typed blocks, a picture and a document distinguished,
        // and the text last.
        let message = Wire::Anthropic.user_message("what is wrong here", &files);
        let content = message["content"].as_array().expect("a block list");
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["type"], "document");
        assert_eq!(content[2]["type"], "text");
        assert_eq!(content[2]["text"], "what is wrong here");

        // OpenAI: a data URI, and the document is not there at all — it was
        // refused, not smuggled in as an image.
        let message = Wire::OpenAi.user_message("what is wrong here", &files);
        let content = message["content"].as_array().expect("a block list");
        assert_eq!(content.len(), 2, "a refused file reached the wire");
        assert_eq!(content[0]["type"], "image_url");
        assert!(content[0]["image_url"]["url"]
            .as_str()
            .expect("a url")
            .starts_with("data:image/png;base64,"));

        // Ollama: content stays a string, images hang beside it, bare.
        let message = Wire::Ollama.user_message("what is wrong here", &files);
        assert_eq!(message["content"], "what is wrong here");
        let images = message["images"].as_array().expect("an image list");
        assert_eq!(images.len(), 1);
        assert!(!images[0].as_str().expect("base64").starts_with("data:"));
    }

    #[cfg(feature = "api")]
    #[test]
    fn a_message_with_nothing_attached_is_the_plain_one() {
        // The path every ordinary prompt takes: unchanged, so attaching a file
        // is the only thing that changes a request.
        for wire in [Wire::Anthropic, Wire::OpenAi, Wire::Ollama, Wire::Read] {
            let message = wire.user_message("just a prompt", &[]);
            assert_eq!(message["content"], "just a prompt", "{wire:?}");
            assert!(message.get("images").is_none(), "{wire:?}");
        }
        // And a CLI agent reads the file itself, so its message never grows
        // blocks even when files are present.
        let message = Wire::Read.user_message("look at bug.png", &[picture()]);
        assert_eq!(message["content"], "look at bug.png");
    }
}
