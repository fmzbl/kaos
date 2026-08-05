//! Reading the open web: search, and fetching a page as text.
//!
//! An agent that can read files and run commands but cannot look anything up is
//! confined to what is already on the disk. This module is the seam through
//! which it reaches outside, and it is deliberately the only one: everything
//! network-facing lives here so the authority question — what may an agent
//! reach, and with whose credentials — has one place to be answered.
//!
//! Two capabilities, kept separate because they fail differently. A **search**
//! turns a question into a ranked list of places to look; a **fetch** turns one
//! of those places into readable text. An agent that can only search reads
//! nothing but summaries; one that can only fetch has to be told where to go.
//!
//! Providers are chosen by configuration rather than compiled in. The keyless
//! default works with no setup at all, and a host that has bought a search API
//! names it in `KAOS_SEARCH` and stores the key with the same credential store
//! the model providers use. Nothing here is specific to any one site.

use std::fmt;
#[cfg(feature = "api")]
use std::time::Duration;

/// How long any single network call may take before it is abandoned.
///
/// A hung request is worse than a failed one: the agent loop is blocking on it,
/// and the reader is looking at a stalled run with no way to tell whether it is
/// working. Both search and fetch are bounded, and the bound is deliberately
/// short — a page that has not answered in this long is not going to.
#[cfg(feature = "api")]
const TIMEOUT: Duration = Duration::from_secs(20);

/// The largest response body that will be read into memory.
///
/// A fetch is a reading tool, not a downloader. Anything past this is a file
/// the agent should be asking the host to handle, and reading it into the
/// prompt would blow the context long before it blew the memory.
#[cfg(feature = "api")]
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// The identity a fetch presents. Some sites refuse an unnamed client outright,
/// and an honest one is better than borrowing a browser's.
#[cfg(feature = "api")]
const USER_AGENT: &str = concat!("kaos/", env!("CARGO_PKG_VERSION"), " (+agent tool)");

/// One thing a search found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    /// The provider's own summary, when it gives one. Empty is normal.
    pub snippet: String,
}

/// Why a lookup could not be answered.
///
/// Deliberately a plain message rather than a taxonomy: every one of these is
/// reported back to the agent as text for it to reason about, and an agent
/// reads "no search provider is configured" better than it reads an enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebError(pub String);

impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WebError {}

fn fail(message: impl Into<String>) -> WebError {
    WebError(message.into())
}

/// Which search service a lookup goes through.
///
/// Selected by `KAOS_SEARCH`, so a host can change where its agents look
/// without a rebuild. The variants differ only in how a query becomes a request
/// and how a response becomes [`Hit`]s; everything else is shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// DuckDuckGo's HTML endpoint. Needs no key and no account, which is why it
    /// is the default: an agent can look something up on a fresh checkout.
    DuckDuckGo,
    /// Brave's Search API. Needs `BRAVE_API_KEY`.
    Brave,
    /// Serper's Google-backed API. Needs `SERPER_API_KEY`.
    Serper,
    /// Tavily, which is built for agent use and returns cleaned text. Needs
    /// `TAVILY_API_KEY`.
    Tavily,
}

impl Provider {
    /// Every provider that can be named in configuration.
    pub const ALL: &'static [Self] = &[Self::DuckDuckGo, Self::Brave, Self::Serper, Self::Tavily];

    /// The name written in `KAOS_SEARCH`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "duckduckgo",
            Self::Brave => "brave",
            Self::Serper => "serper",
            Self::Tavily => "tavily",
        }
    }

    /// The credential this provider needs, or `None` when it needs none.
    pub const fn key_var(self) -> Option<&'static str> {
        match self {
            Self::DuckDuckGo => None,
            Self::Brave => Some("BRAVE_API_KEY"),
            Self::Serper => Some("SERPER_API_KEY"),
            Self::Tavily => Some("TAVILY_API_KEY"),
        }
    }

    /// Read a configured name, accepting the obvious spellings.
    ///
    /// An unknown name is not silently replaced by the default: a host that
    /// asked for a provider it misspelled should be told, not quietly sent
    /// somewhere else.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "" | "duckduckgo" | "ddg" | "duck" => Some(Self::DuckDuckGo),
            "brave" => Some(Self::Brave),
            "serper" | "google" => Some(Self::Serper),
            "tavily" => Some(Self::Tavily),
            _ => None,
        }
    }
}

/// Percent-encode one query for use in a URL.
///
/// Written out rather than pulled in: the unreserved set is four lines, and a
/// dependency for it would be a dependency in the network path.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Strip HTML to the text a reader would see.
///
/// Script and style bodies go entirely — their contents are not prose and are
/// often larger than the page. Block-level tags become newlines so structure
/// survives, entities are decoded, and runs of blank space collapse. This is
/// not a parser and does not try to be: it is the smallest thing that turns a
/// page into something worth putting in a prompt.
pub fn to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    // Drop whole elements whose text content is never prose.
    for tag in ["script", "style", "noscript", "svg", "head"] {
        rest = rest.trim_start();
        let mut cleaned = String::with_capacity(rest.len());
        let mut cursor = rest;
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        while let Some(start) = cursor.to_ascii_lowercase().find(&open) {
            cleaned.push_str(&cursor[..start]);
            let after = &cursor[start..];
            match after.to_ascii_lowercase().find(&close) {
                Some(end) => cursor = &after[end + close.len()..],
                None => {
                    cursor = "";
                    break;
                }
            }
        }
        cleaned.push_str(cursor);
        rest = Box::leak(cleaned.into_boxed_str());
    }

    let mut in_tag = false;
    let mut tag = String::new();
    for character in rest.chars() {
        match character {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if matches!(
                    name.as_str(),
                    "p" | "br"
                        | "div"
                        | "li"
                        | "tr"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "blockquote"
                        | "pre"
                        | "table"
                ) {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(character),
            other => out.push(other),
        }
    }
    collapse(&decode_entities(&out))
}

/// Decode the handful of entities that actually appear in prose.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find(';').filter(|end| *end <= 12) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let entity = &after[1..end];
        let decoded = match entity {
            "amp" => Some("&".to_string()),
            "lt" => Some("<".to_string()),
            "gt" => Some(">".to_string()),
            "quot" => Some("\"".to_string()),
            "apos" | "#39" => Some("'".to_string()),
            "nbsp" => Some(" ".to_string()),
            "hellip" => Some("…".to_string()),
            "mdash" => Some("—".to_string()),
            "ndash" => Some("–".to_string()),
            numeric if numeric.starts_with('#') => numeric
                .trim_start_matches('#')
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .map(|c| c.to_string()),
            _ => None,
        };
        match decoded {
            Some(text) => {
                out.push_str(&text);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Collapse the whitespace an HTML strip leaves behind.
fn collapse(text: &str) -> String {
    let mut lines = Vec::new();
    let mut blank = 0usize;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank += 1;
            // One blank line is a paragraph break; more is leftover markup.
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

/// Keep a body within what a prompt can carry, saying so where it was cut.
pub fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    let boundary = kept.rfind('\n').unwrap_or(kept.len());
    format!(
        "{}\n\n[… truncated at {max_chars} characters; ask for a narrower page or a specific section]",
        &kept[..boundary]
    )
}

/// The request URL and headers for one search.
///
/// Separated from sending it so the shape of every provider's request is
/// testable without a network.
pub fn request_for(
    provider: Provider,
    query: &str,
    key: Option<&str>,
) -> (String, Vec<(String, String)>, Option<String>) {
    match provider {
        Provider::DuckDuckGo => (
            format!("https://html.duckduckgo.com/html/?q={}", encode(query)),
            Vec::new(),
            None,
        ),
        Provider::Brave => (
            format!(
                "https://api.search.brave.com/res/v1/web/search?q={}",
                encode(query)
            ),
            vec![
                ("Accept".into(), "application/json".into()),
                (
                    "X-Subscription-Token".into(),
                    key.unwrap_or_default().to_string(),
                ),
            ],
            None,
        ),
        Provider::Serper => (
            "https://google.serper.dev/search".into(),
            vec![
                ("X-API-KEY".into(), key.unwrap_or_default().to_string()),
                ("Content-Type".into(), "application/json".into()),
            ],
            Some(format!("{{\"q\":{}}}", json_string(query))),
        ),
        Provider::Tavily => (
            "https://api.tavily.com/search".into(),
            vec![("Content-Type".into(), "application/json".into())],
            Some(format!(
                "{{\"api_key\":{},\"query\":{},\"max_results\":8}}",
                json_string(key.unwrap_or_default()),
                json_string(query)
            )),
        ),
    }
}

/// Quote a string as a JSON scalar, escaping what must be escaped.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Pull the hits out of DuckDuckGo's HTML.
///
/// Their result links are wrapped in a redirect whose real destination is in a
/// `uddg=` parameter, so the encoded target is unwrapped rather than handed to
/// the agent as a tracking URL it would then fetch.
pub fn parse_duckduckgo(html: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for chunk in html.split("result__a").skip(1) {
        let Some(href_at) = chunk.find("href=\"") else {
            continue;
        };
        let after = &chunk[href_at + 6..];
        let Some(end) = after.find('"') else { continue };
        let url = unwrap_redirect(&decode_entities(&after[..end]));
        let title = after
            .find('>')
            .map(|open| to_text(&after[open + 1..]))
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        let snippet = chunk
            .find("result__snippet")
            .and_then(|at| chunk[at..].find('>').map(|open| at + open + 1))
            .map(|start| to_text(&chunk[start..]))
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        if !url.is_empty() && !title.is_empty() {
            hits.push(Hit {
                title,
                url,
                snippet,
            });
        }
    }
    hits
}

/// Unwrap a search redirect to the destination it is standing in for.
fn unwrap_redirect(url: &str) -> String {
    let Some(at) = url.find("uddg=") else {
        return url.trim_start_matches("//").to_string().pipe_https();
    };
    let encoded = &url[at + 5..];
    let encoded = encoded.split('&').next().unwrap_or(encoded);
    percent_decode(encoded)
}

trait PipeHttps {
    fn pipe_https(self) -> String;
}

impl PipeHttps for String {
    /// A protocol-relative URL is not fetchable as written.
    fn pipe_https(self) -> String {
        if self.starts_with("http://") || self.starts_with("https://") || self.is_empty() {
            self
        } else {
            format!("https://{self}")
        }
    }
}

/// Decode `%XX` escapes back to text.
pub fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Pull hits out of a JSON search response without a JSON parser.
///
/// The three keyed providers return different envelopes around the same three
/// fields, so the shapes are named rather than decoded: whichever key holds the
/// destination, the title, and the summary. Written by hand because this crate
/// builds without `serde_json` unless the `api` feature is on, and a search
/// tool that disappears with a feature flag is not a tool.
pub fn parse_json_hits(body: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut rest = body;
    while let Some(at) = rest.find("\"url\"").or_else(|| rest.find("\"link\"")) {
        let window_end = rest[at..]
            .find("},")
            .map_or(rest.len(), |end| (at + end).min(rest.len()));
        let window = &rest[at.saturating_sub(400)..window_end];
        let url = json_field(window, "url")
            .or_else(|| json_field(window, "link"))
            .unwrap_or_default();
        let title = json_field(window, "title").unwrap_or_default();
        let snippet = json_field(window, "description")
            .or_else(|| json_field(window, "snippet"))
            .or_else(|| json_field(window, "content"))
            .unwrap_or_default();
        if url.starts_with("http") && !title.is_empty() {
            hits.push(Hit {
                title,
                url,
                snippet,
            });
        }
        rest = &rest[window_end.max(at + 5)..];
    }
    hits
}

/// Read one string field out of a JSON object fragment.
fn json_field(window: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = window.find(&needle)?;
    let after = window[at + needle.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    let body = after.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(character) = chars.next() {
        match character {
            '"' => return Some(out),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(decoded) =
                        u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                    {
                        out.push(decoded);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            other => out.push(other),
        }
    }
    None
}

/// The provider a lookup will use, and the credential it needs.
///
/// Reads `KAOS_SEARCH`, then the environment, then the stored credentials, so a
/// key set for the session wins over one on disk — the same precedence the
/// model providers use.
pub fn configured() -> Result<(Provider, Option<String>), WebError> {
    let name = kaos_core::config::value("KAOS_SEARCH").unwrap_or_default();
    let provider = Provider::parse(&name).ok_or_else(|| {
        fail(format!(
            "unknown search provider '{name}' — try {}",
            Provider::ALL
                .iter()
                .map(|p| p.name())
                .collect::<Vec<_>>()
                .join(" | ")
        ))
    })?;
    let Some(var) = provider.key_var() else {
        return Ok((provider, None));
    };
    // `auth::load` seeds the environment from the credential store at startup,
    // so one read covers both an exported key and a stored one.
    let key = std::env::var(var)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            fail(format!(
                "{} needs {var}. Set it, or choose a provider that needs no key \
                 with KAOS_SEARCH=duckduckgo",
                provider.name()
            ))
        })?;
    Ok((provider, Some(key)))
}

/// Look a question up on the configured provider.
///
/// # Errors
///
/// Returns the provider's own failure, or a description of what is missing when
/// the host has not configured one.
pub fn search(query: &str) -> Result<Vec<Hit>, WebError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(fail("a search needs a query"));
    }
    let (provider, key) = configured()?;
    let (url, headers, body) = request_for(provider, query, key.as_deref());
    let response = send(&url, &headers, body.as_deref())?;
    let hits = match provider {
        Provider::DuckDuckGo => parse_duckduckgo(&response),
        _ => parse_json_hits(&response),
    };
    if hits.is_empty() {
        return Err(fail(format!(
            "{} returned nothing for {query:?}",
            provider.name()
        )));
    }
    Ok(hits)
}

/// Fetch one page and return it as readable text.
///
/// # Errors
///
/// Returns a description of what went wrong: a refused scheme, an unreachable
/// host, or a response the page could not be read out of.
pub fn fetch(url: &str) -> Result<String, WebError> {
    let url = url.trim();
    // Only the web. `file://` would turn a reading tool into an unaudited file
    // read, and the agent already has one of those that the host can gate.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(fail(format!(
            "fetch takes an http:// or https:// URL, not {url:?}"
        )));
    }
    let body = send(url, &[], None)?;
    let text = if body.trim_start().starts_with('<') {
        to_text(&body)
    } else {
        collapse(&body)
    };
    if text.trim().is_empty() {
        return Err(fail(format!("{url} returned nothing readable")));
    }
    Ok(text)
}

#[cfg(feature = "api")]
fn send(url: &str, headers: &[(String, String)], body: Option<&str>) -> Result<String, WebError> {
    use std::io::Read as _;

    let agent = ureq::agent();
    let mut request = match body {
        Some(_) => agent.post(url),
        None => agent.get(url),
    }
    .timeout(TIMEOUT)
    .set("User-Agent", USER_AGENT);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = match body {
        Some(body) => request.send_string(body),
        None => request.call(),
    }
    .map_err(|error| fail(format!("{url}: {error}")))?;

    // Read with a ceiling rather than to the end: a reading tool must not be
    // the thing that exhausts memory on a page that turns out to be a download.
    let mut text = String::new();
    response
        .into_reader()
        .take(MAX_BYTES as u64)
        .read_to_string(&mut text)
        .map_err(|error| fail(format!("{url}: could not read the response: {error}")))?;
    Ok(text)
}

#[cfg(not(feature = "api"))]
fn send(
    _url: &str,
    _headers: &[(String, String)],
    _body: Option<&str>,
) -> Result<String, WebError> {
    Err(fail(
        "this build has no HTTP support — rebuild with the `api` feature to let \
         agents reach the web",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_is_reachable_by_the_name_it_documents() {
        for provider in Provider::ALL {
            assert_eq!(Provider::parse(provider.name()), Some(*provider));
        }
        // Spellings a host is likely to write.
        assert_eq!(Provider::parse("DDG"), Some(Provider::DuckDuckGo));
        assert_eq!(Provider::parse("google"), Some(Provider::Serper));
        // The keyless one is what an unset configuration means.
        assert_eq!(Provider::parse(""), Some(Provider::DuckDuckGo));
        // A misspelling is refused rather than silently redirected.
        assert_eq!(Provider::parse("brav"), None);
        // Only the keyless provider works with no credential.
        assert_eq!(Provider::DuckDuckGo.key_var(), None);
        for provider in Provider::ALL.iter().filter(|p| **p != Provider::DuckDuckGo) {
            assert!(provider.key_var().is_some(), "{provider:?} needs a key");
        }
    }

    #[test]
    fn a_query_becomes_a_request_without_leaking_the_key_into_the_url() {
        for provider in Provider::ALL {
            let (url, headers, body) =
                request_for(*provider, "rust lifetime elision", Some("SECRET"));
            assert!(url.starts_with("https://"), "{provider:?} is not https");
            assert!(
                !url.contains("SECRET"),
                "{provider:?} put the credential in the URL, where it lands in logs"
            );
            match provider.key_var() {
                // A key travels in a header or a body, never a query string.
                Some(_) => assert!(
                    headers.iter().any(|(_, v)| v == "SECRET")
                        || body.as_deref().is_some_and(|b| b.contains("SECRET")),
                    "{provider:?} never sent the key"
                ),
                None => assert!(headers.is_empty() && body.is_none()),
            }
        }
        // Spaces and punctuation survive the trip.
        let (url, _, _) = request_for(Provider::DuckDuckGo, "a & b?", None);
        assert!(url.ends_with("q=a+%26+b%3F"), "{url}");
    }

    #[test]
    fn a_page_becomes_the_text_a_reader_would_see() {
        let html = "<html><head><title>x</title><style>p{color:red}</style></head>\
                    <body><script>alert('no')</script>\
                    <h1>Title</h1><p>First &amp; second.</p>\
                    <div>Third&nbsp;line</div><p></p><p>Fourth</p></body></html>";
        let text = to_text(html);
        // Script, style and head contents are gone entirely.
        assert!(!text.contains("alert"), "{text}");
        assert!(!text.contains("color:red"), "{text}");
        // Prose survives, entities are decoded, structure becomes newlines.
        assert!(text.contains("First & second."), "{text}");
        assert!(text.contains("Third line"), "{text}");
        assert!(text.contains("Title"), "{text}");
        // No run of blank lines is left behind.
        assert!(!text.contains("\n\n\n"), "{text:?}");
    }

    #[test]
    fn a_search_result_is_read_back_with_its_real_destination() {
        // DuckDuckGo wraps every hit in a redirect; the agent must be handed
        // the page, not the tracker.
        let html = r#"
          <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F&amp;rut=x">
            The Rust Book</a>
          <a class="result__snippet">An introduction to Rust.</a>
        "#;
        let hits = parse_duckduckgo(html);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].url, "https://doc.rust-lang.org/book/");
        assert_eq!(hits[0].title, "The Rust Book");
    }

    #[test]
    fn a_json_response_is_read_whatever_envelope_it_arrives_in() {
        // Brave/Serper/Tavily disagree on the key names; the same three fields
        // are read out of all of them.
        let brave = r#"{"web":{"results":[
            {"title":"Ownership","url":"https://example.com/own","description":"Moves and borrows"},
            {"title":"Lifetimes","url":"https://example.com/life","description":"Elision rules"}]}}"#;
        let hits = parse_json_hits(brave);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].url, "https://example.com/own");
        assert_eq!(hits[1].title, "Lifetimes");

        let serper =
            r#"{"organic":[{"title":"A","link":"https://example.com/a","snippet":"one"}]}"#;
        let hits = parse_json_hits(serper);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].snippet, "one");

        // Escapes inside a field do not end it early.
        let escaped = r#"{"results":[{"title":"He said \"hi\"","url":"https://example.com/q","content":"a\nb"}]}"#;
        let hits = parse_json_hits(escaped);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].title, "He said \"hi\"");
    }

    #[test]
    fn a_long_page_is_clipped_where_it_says_it_is() {
        let text = (0..500)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let clipped = clip(&text, 200);
        assert!(clipped.chars().count() < text.chars().count());
        assert!(clipped.contains("truncated at 200 characters"), "{clipped}");
        // Short text is returned untouched, with no notice bolted on.
        assert_eq!(clip("short", 200), "short");
    }
}
