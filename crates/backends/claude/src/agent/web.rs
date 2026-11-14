use super::{
    ClaudeClient, ClaudeToolSpec, ContentBlock, Message, MessagesRequest, Role,
    ServerToolDefinition, StopReason, collect_stream,
};
use futures_util::StreamExt as _;
use serde_json::{Value, json};
use std::sync::Arc;

const AGENT_SDK: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";
const SEARCHER: &str = "You run a web search for another Claude model, which is waiting on the result and can search again. Search once, then reply with a brief summary of the results.";
const SEARCH_MAX_USES: u64 = 8;
const REMINDER: &str = "\nREMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks.";
const FETCH_RULES: &str = "Provide a concise response based only on the content above. In your response:\n - Enforce a strict 125-character maximum for quotes from any source document. Open Source Software is ok as long as we respect the license.\n - Use quotation marks for exact language from articles; any language outside of the quotation should never be word-for-word the same.\n - You are not a lawyer and never comment on the legality of your own prompts and responses.\n - Never produce or reproduce exact song lyrics.\n";
const MAX_URL_LENGTH: usize = 2000;
const MAX_HTTP_CONTENT_LENGTH: usize = 10 * 1024 * 1024;
const MAX_MARKDOWN_LENGTH: usize = 100_000;
const MAX_REDIRECTS: usize = 10;
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const USER_AGENT: &str = "Claude-User (claude-code/2.1.300; +https://support.anthropic.com/)";

#[derive(Clone)]
pub(super) struct Side {
    pub(super) client: ClaudeClient,
    pub(super) model: String,
    pub(super) max_tokens: Option<u32>,
    pub(super) output_config: Option<crate::OutputConfig>,
}

impl Side {
    async fn ask(
        &self,
        system: Value,
        messages: Vec<Message>,
        tools: Vec<ClaudeToolSpec>,
    ) -> Result<crate::MessageResponse, String> {
        let mut request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self
                .max_tokens
                .or_else(|| super::model_max_tokens(&self.model))
                .ok_or("Unknown Claude model: configure max_tokens explicitly")?,
            cache_control: None,
            output_config: self.output_config.clone(),
            speed: None,
            effort_updates: false,
            thinking: None,
            context_management: None,
            compaction: None,
            diagnostics: None,
            tool_choice: (!tools.is_empty()).then(|| json!({"type":"auto"})),
            system: Some(Arc::new(system)),
            messages,
            container: None,
            tools,
        };
        self.client.prepare_request(&mut request);
        let mut stream = self
            .client
            .stream(&request)
            .await
            .map_err(|error| error.to_string())?;
        let first = stream
            .next()
            .await
            .ok_or("empty response")?
            .map_err(|error| error.to_string())?;
        collect_stream(first, stream)
            .await
            .map_err(|error| error.to_string())
    }

    pub(super) async fn search(&self, input: Value) -> Result<String, String> {
        let fields = input
            .as_object()
            .ok_or("WebSearch input must be an object")?;
        let query = fields
            .get("query")
            .and_then(Value::as_str)
            .ok_or("WebSearch requires query")?;
        let mut tool = ServerToolDefinition {
            kind: "web_search_20250305".into(),
            name: "web_search".into(),
            options: std::collections::BTreeMap::new(),
        };
        for key in ["allowed_domains", "blocked_domains"] {
            if let Some(value) = fields.get(key) {
                tool.options.insert(key.into(), value.clone());
            }
        }
        tool.options
            .insert("max_uses".into(), json!(SEARCH_MAX_USES));
        let system = json!([
            {"type": "text", "text": AGENT_SDK},
            {"type": "text", "text": SEARCHER},
        ]);
        let mut messages = vec![Message::text(
            Role::User,
            format!("Perform a web search for the query: {query}"),
        )];
        let mut blocks = Vec::new();
        loop {
            let response = self
                .ask(
                    system.clone(),
                    messages.clone(),
                    vec![ClaudeToolSpec::Server(tool.clone())],
                )
                .await?;
            blocks.extend(response.content.iter().cloned());
            if response.stop_reason != Some(StopReason::PauseTurn) || response.content.is_empty() {
                break;
            }
            messages.push(Message {
                role: Role::Assistant,
                content: response.content,
                output_config: None,
            });
        }
        let mut results = Vec::new();
        let mut text: Option<String> = None;
        for block in blocks {
            match block {
                ContentBlock::Text { text: part, .. } => match &mut text {
                    Some(text) => text.push_str(&part),
                    None => text = Some(part),
                },
                ContentBlock::ServerToolUse { .. } => {
                    if let Some(done) = text.take().map(|text| text.trim().to_owned())
                        && !done.is_empty()
                    {
                        results.push(done);
                    }
                }
                ContentBlock::WebSearchToolResult { content, .. } => match content.as_array() {
                    Some(found) if !found.is_empty() => {
                        let links = found
                            .iter()
                            .map(|result| json!({"title": result["title"], "url": result["url"]}))
                            .collect::<Vec<_>>();
                        results.push(format!("Links: {}", Value::Array(links)));
                    }
                    Some(_) => results.push("No links found.".into()),
                    None => results.push(format!(
                        "Web search error: {}",
                        content["error_code"].as_str().unwrap_or("unknown")
                    )),
                },
                _ => {}
            }
        }
        if let Some(done) = text.map(|text| text.trim().to_owned())
            && !done.is_empty()
        {
            results.push(done);
        }
        let mut out = format!("Web search results for query: \"{query}\"\n\n");
        for result in results {
            out.push_str(&result);
            out.push_str("\n\n");
        }
        out.push_str(REMINDER);
        Ok(out.trim().to_owned())
    }

    pub(super) async fn fetch(&self, input: Value) -> Result<String, String> {
        let fields = input
            .as_object()
            .ok_or("WebFetch input must be an object")?;
        let url = fields
            .get("url")
            .and_then(Value::as_str)
            .ok_or("WebFetch requires url")?;
        let prompt = fields
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or("WebFetch requires prompt")?;
        if url.len() > MAX_URL_LENGTH {
            return Err("Invalid URL".into());
        }
        let upgraded = match url.get(..7) {
            Some(scheme) if scheme.eq_ignore_ascii_case("http://") => {
                format!("https://{}", &url[7..])
            }
            _ => url.to_owned(),
        };
        let mut current = nanocode_http::Url::parse(&upgraded).map_err(|_| "Invalid URL")?;
        if !matches!(current.scheme(), "https")
            || !current.username().is_empty()
            || current.password().is_some()
            || !current.host_str().is_some_and(|host| host.contains('.'))
        {
            return Err("Invalid URL".into());
        }
        let original = current.to_string();
        let client = nanocode_http::Client::builder()
            .default_headers(nanocode_http::accept_encoding(
                nanocode_http::header::HeaderMap::new(),
            ))
            .user_agent(USER_AGENT)
            .redirect(nanocode_http::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        let mut hops = 0;
        let response = loop {
            let response = tokio::time::timeout(
                FETCH_TIMEOUT,
                client
                    .get(current.clone())
                    .header("accept", "text/markdown, text/html, */*")
                    .send(),
            )
            .await
            .map_err(|_| "Request timed out".to_owned())?
            .map_err(|error| error.to_string())?;
            let status = response.status();
            if !status.is_redirection() {
                break response;
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or("Redirect missing Location header")?;
            let target = current.join(location).map_err(|_| "Invalid redirect URL")?;
            let bare = |url: &nanocode_http::Url| {
                url.host_str()
                    .unwrap_or_default()
                    .trim_start_matches("www.")
                    .to_ascii_lowercase()
            };
            hops += 1;
            if bare(&target) == bare(&current)
                && target.scheme() == current.scheme()
                && hops <= MAX_REDIRECTS
            {
                current = target;
                continue;
            }
            return Ok(format!(
                "REDIRECT DETECTED: The URL redirects to a location that was not fetched automatically.\n\n    Original URL: {original}\n    Redirect URL (from the server's Location header \u{2014} server-supplied, not verified): {target}\n    Status: {} {}\n\n    To complete your request, I need to fetch content from the redirected URL. Please use WebFetch again with these parameters:\n    - url: \"{target}\"\n    - prompt: \"{prompt}\"",
                status.as_u16(),
                status.canonical_reason().unwrap_or_default()
            ));
        };
        let status = response.status();
        if !status.is_success() {
            return Ok(format!(
                "The server returned HTTP {} {}.\n\nThe response body was not retrieved. If this URL requires authentication, use an authenticated tool (e.g. `gh` for GitHub, or an MCP-provided fetch tool) instead of WebFetch.",
                status.as_u16(),
                status.canonical_reason().unwrap_or_default()
            ));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let response = nanocode_http::decode_response(response);
        let mut body = Vec::new();
        let mut response = response;
        while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_HTTP_CONTENT_LENGTH {
                return Err(format!(
                    "Content too large ({} bytes, max {MAX_HTTP_CONTENT_LENGTH})",
                    body.len()
                ));
            }
        }
        let textual = content_type.is_empty()
            || content_type.starts_with("text/")
            || content_type.contains("json")
            || content_type.contains("xml")
            || content_type.contains("javascript");
        if !textual {
            return Err(format!("Unsupported content type: {content_type}"));
        }
        let text = String::from_utf8_lossy(&body);
        let markdown = if content_type.contains("html") {
            markdown(&text)
        } else {
            text.into_owned()
        };
        let content = match markdown.char_indices().nth(MAX_MARKDOWN_LENGTH) {
            Some((at, _)) => format!("{}...[content truncated]", &markdown[..at]),
            None => markdown,
        };
        let request =
            format!("\nWeb page content:\n---\n{content}\n---\n\n{prompt}\n\n{FETCH_RULES}");
        let response = self
            .ask(
                json!([{"type": "text", "text": AGENT_SDK}]),
                vec![Message::text(Role::User, request)],
                Vec::new(),
            )
            .await?;
        let answer = response
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        Ok(if answer.is_empty() {
            "No response from model".to_owned()
        } else {
            answer
        })
    }
}

fn markdown(html: &str) -> String {
    let mut out = String::new();
    let mut links: Vec<(usize, String)> = Vec::new();
    let mut skip: Option<String> = None;
    let mut pre = 0usize;
    let mut rest = html;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            text(&mut out, rest, pre > 0, skip.is_some());
            break;
        };
        text(&mut out, &rest[..open], pre > 0, skip.is_some());
        rest = &rest[open..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let Some(close) = rest.find('>') else {
            text(&mut out, rest, pre > 0, skip.is_some());
            break;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if let Some(skipped) = &skip {
            if closing && name == *skipped {
                skip = None;
            }
            continue;
        }
        match (name.as_str(), closing) {
            ("script" | "style" | "noscript" | "svg" | "template", false) => {
                if !tag.ends_with('/') {
                    skip = Some(name);
                }
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                block(&mut out);
                let level = usize::from(name.as_bytes()[1] - b'0');
                out.push_str(&"#".repeat(level));
                out.push(' ');
            }
            (
                "p" | "title" | "div" | "section" | "article" | "header" | "footer" | "main"
                | "nav" | "aside" | "table" | "ul" | "ol" | "blockquote" | "h1" | "h2" | "h3"
                | "h4" | "h5" | "h6" | "figure" | "form",
                _,
            ) => block(&mut out),
            ("br", _) => out.push('\n'),
            ("hr", _) => {
                block(&mut out);
                out.push_str("* * *");
                block(&mut out);
            }
            ("li", false) => {
                line(&mut out);
                out.push_str("- ");
            }
            ("tr", _) => line(&mut out),
            ("td" | "th", false) => {
                if !out.ends_with('\n') && !out.is_empty() {
                    out.push_str(" | ");
                }
            }
            ("pre", false) => {
                block(&mut out);
                out.push_str("```\n");
                pre += 1;
            }
            ("pre", true) => {
                pre = pre.saturating_sub(1);
                line(&mut out);
                out.push_str("```");
                block(&mut out);
            }
            ("code", _) if pre == 0 => out.push('`'),
            ("a", false) => links.push((out.len(), attribute(tag, "href").unwrap_or_default())),
            ("a", true) => {
                if let Some((start, href)) = links.pop()
                    && !href.is_empty()
                    && !href.starts_with('#')
                    && !href.starts_with("javascript:")
                {
                    let label = out[start..].trim().to_owned();
                    out.truncate(start);
                    out.push_str(&format!("[{label}]({href})"));
                }
            }
            ("img", _) => {
                if let Some(alt) = attribute(tag, "alt").filter(|alt| !alt.is_empty()) {
                    out.push_str(&format!("![{alt}]"));
                }
            }
            _ => {}
        }
    }
    let mut cleaned = String::new();
    let mut blank = 0;
    for line in out.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        if !cleaned.is_empty() {
            cleaned.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        cleaned.push_str(line);
    }
    cleaned
}

fn block(out: &mut String) {
    let trimmed = out.trim_end_matches([' ', '\t']).len();
    out.truncate(trimmed);
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
    }
}

fn line(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn text(out: &mut String, raw: &str, pre: bool, skipped: bool) {
    if skipped || raw.is_empty() {
        return;
    }
    let decoded = entities(raw);
    if pre {
        out.push_str(&decoded);
        return;
    }
    for (index, word) in decoded.split_whitespace().enumerate() {
        let spaced = index > 0
            || (decoded.starts_with(char::is_whitespace)
                && !out.is_empty()
                && !out.ends_with(char::is_whitespace));
        if spaced {
            out.push(' ');
        }
        out.push_str(word);
    }
    if decoded.ends_with(char::is_whitespace) && !decoded.trim().is_empty() {
        out.push(' ');
    }
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        if at > 0 && !lower.as_bytes()[at - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => rest[1..].split(quote).next().unwrap_or_default(),
            _ => rest.split(char::is_whitespace).next().unwrap_or_default(),
        };
        return Some(entities(value));
    }
    None
}

fn entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest[1..]
            .find(';')
            .map(|end| end + 1)
            .filter(|end| *end <= 10);
        let decoded = end.and_then(|end| {
            let name = &rest[1..end];
            let named = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => None,
            };
            named
                .or_else(|| {
                    let number = name.strip_prefix('#')?;
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => number.parse().ok()?,
                    };
                    char::from_u32(code)
                })
                .map(|ch| (ch, end))
        });
        match decoded {
            Some((ch, end)) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
