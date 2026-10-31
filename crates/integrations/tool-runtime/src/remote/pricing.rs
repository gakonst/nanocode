use std::{collections::HashMap, path::Path, sync::Arc};

use nanocode_sync::Atom;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{HandCapabilities, HandCapability, HandPermission};
use crate::embedded::{CodeModeHostError, HostFuture};

type Price = dyn Fn(&PricedOperation) -> Result<Option<f64>, String> + Send + Sync;

#[derive(Clone)]
pub struct HandPricing {
    describe: Option<String>,
    payee: String,
    price: Arc<Price>,
}

#[derive(Clone, Debug)]
pub struct PricedOperation {
    pub access: &'static str,
    pub agent: Option<String>,
    pub command: Option<String>,
    pub id: u64,
    pub input: Value,
    pub kind: String,
    pub max: Option<f64>,
    pub paths: Vec<String>,
    pub session: Option<String>,
    pub source: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesFile {
    #[serde(default)]
    describe: Option<String>,
    payee: String,
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    #[serde(default)]
    ops: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    usd: f64,
}

impl HandPricing {
    pub fn new(
        payee: impl Into<String>,
        price: impl Fn(&PricedOperation) -> Result<Option<f64>, String> + Send + Sync + 'static,
    ) -> Result<Self, CodeModeHostError> {
        let payee = payee.into();
        let address = payee
            .strip_prefix("0x")
            .is_some_and(|hex| hex.len() == 40 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
        if !address {
            return Err(CodeModeHostError::new(
                "the pricing payee must be a 0x-prefixed 20-byte address",
            ));
        }
        Ok(Self {
            describe: None,
            payee,
            price: Arc::new(price),
        })
    }

    #[must_use]
    pub fn describe(mut self, text: impl Into<String>) -> Self {
        self.describe = Some(text.into());
        self
    }

    pub fn rules(text: &str) -> Result<Self, CodeModeHostError> {
        let file: RulesFile = serde_json::from_str(text)
            .map_err(|issue| CodeModeHostError::new(format!("invalid pricing file: {issue}")))?;
        if let Some(rule) = file
            .rules
            .iter()
            .find(|rule| !rule.usd.is_finite() || rule.usd < 0.0)
        {
            return Err(CodeModeHostError::new(format!(
                "invalid pricing file: a rule's usd must be a nonnegative number, not {}",
                rule.usd
            )));
        }
        let rules = file.rules;
        let pricing = Self::new(file.payee, move |operation| {
            Ok(rules
                .iter()
                .find(|rule| rule.matches(operation))
                .map(|rule| rule.usd))
        })?;
        Ok(match file.describe {
            Some(text) => pricing.describe(text),
            None => pricing,
        })
    }

    pub(super) fn capability(&self) -> HandCapability {
        let summary = self
            .describe
            .as_deref()
            .map_or_else(String::new, |text| format!("{text} "));
        HandCapability {
            name: "pricing".into(),
            description: format!(
                "{summary}This hand charges in USD for the operations it prices: the brain bills each charge to whoever pays for the agent before the result arrives, and refuses an operation that costs more than the request allows before it runs."
            ),
            tools: Vec::new(),
            permission: HandPermission::Granted,
        }
    }
}

impl Rule {
    fn matches(&self, operation: &PricedOperation) -> bool {
        let named = self.ops.is_empty()
            || self.ops.iter().any(|op| {
                op == &operation.kind
                    || op == operation.access
                    || (op == "read" && operation.access == "run" && !operation.paths.is_empty())
            });
        let touched = self.paths.is_empty()
            || operation.paths.iter().any(|path| {
                self.paths.iter().any(|pattern| {
                    glob(pattern.as_bytes(), path.as_bytes())
                        || (operation.access == "run" && covers(path, pattern))
                })
            });
        named && touched
    }
}

fn covers(directory: &str, pattern: &str) -> bool {
    let fixed = pattern
        .find(['*', '?'])
        .map_or(pattern, |at| &pattern[..at]);
    directory == "."
        || fixed
            .strip_prefix(directory)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern {
        [] => text.is_empty(),
        [b'*', b'*', rest @ ..] => match rest.strip_prefix(b"/") {
            Some(rest) => (0..=text.len())
                .any(|at| (at == 0 || text[at - 1] == b'/') && glob(rest, &text[at..])),
            None => (0..=text.len()).any(|at| glob(rest, &text[at..])),
        },
        [b'*', rest @ ..] => (0..=text.len())
            .take_while(|&at| at == 0 || text[at - 1] != b'/')
            .any(|at| glob(rest, &text[at..])),
        [b'?', rest @ ..] => {
            text.first().is_some_and(|byte| *byte != b'/') && glob(rest, &text[1..])
        }
        [byte, rest @ ..] => text.first() == Some(byte) && glob(rest, &text[1..]),
    }
}

pub(super) struct Priced {
    pub(super) capability: HandCapability,
    pub(super) inner: Option<Arc<dyn HandCapabilities>>,
}

impl HandCapabilities for Priced {
    fn capabilities(&self) -> HostFuture<'_, Vec<HandCapability>> {
        Box::pin(async move {
            let mut listed = match &self.inner {
                Some(inner) => inner.capabilities().await,
                None => Vec::new(),
            };
            listed.push(self.capability.clone());
            listed
        })
    }

    fn request<'a>(
        &'a self,
        name: &'a str,
        reason: &'a str,
    ) -> HostFuture<'a, Result<HandPermission, String>> {
        Box::pin(async move {
            if name == self.capability.name {
                return Ok(HandPermission::Granted);
            }
            match &self.inner {
                Some(inner) => inner.request(name, reason).await,
                None => Err(format!("this hand offers no capability named `{name}`")),
            }
        })
    }
}

#[derive(Clone)]
struct Quote {
    charge: String,
    resumable: bool,
}

pub(super) struct Meter {
    cwd: String,
    pricing: HandPricing,
    quotes: Atom<HashMap<u64, Quote>>,
}

pub(super) enum Screened {
    Answer(String),
    Pass,
}

impl Meter {
    pub(super) fn new(pricing: HandPricing, cwd: &Path) -> Arc<Self> {
        Arc::new(Self {
            cwd: cwd.to_string_lossy().trim_end_matches('/').to_owned(),
            pricing,
            quotes: Atom::default(),
        })
    }

    pub(super) fn catalog(&self, text: String) -> String {
        let advertised = match &self.pricing.describe {
            Some(describe) => json!({ "describe": describe, "payee": self.pricing.payee }),
            None => json!({ "payee": self.pricing.payee }),
        };
        append(text, "pricing", &advertised.to_string())
    }

    pub(super) fn screen(&self, text: &str) -> Screened {
        if text.contains("\"type\":\"ack\"") {
            if let Ok(frame) = serde_json::from_str::<Value>(text)
                && let Some(ids) = frame.get("ids").and_then(Value::as_array)
            {
                let ids = ids.iter().filter_map(Value::as_u64).collect::<Vec<_>>();
                self.quotes.update(|quotes| {
                    for id in &ids {
                        quotes.remove(id);
                    }
                });
            }
            return Screened::Pass;
        }
        if !text.contains("\"type\":\"request\"") {
            return Screened::Pass;
        }
        let Ok(frame) = serde_json::from_str::<Value>(text) else {
            return Screened::Pass;
        };
        let Some(operation) = operation(&frame, &self.cwd) else {
            return Screened::Pass;
        };
        let refuse = |reason: String| {
            Screened::Answer(
                json!({ "type": "reply", "id": operation.id, "result": { "Err": reason } })
                    .to_string(),
            )
        };
        let usd = match (self.pricing.price)(&operation) {
            Ok(Some(usd)) if usd != 0.0 => usd,
            Ok(_) => return Screened::Pass,
            Err(issue) => {
                return refuse(format!(
                    "the hand refused this operation: {issue}; nothing ran"
                ));
            }
        };
        if !usd.is_finite() || usd < 0.0 {
            return refuse(
                "the hand priced this operation at an invalid amount; nothing ran".into(),
            );
        }
        let usd = (usd * 1e6).round().max(1.0) / 1e6;
        let Some(max) = operation.max else {
            return refuse(format!(
                "this hand charges {} for this operation, and the brain that sent it does not bill hand charges; nothing ran",
                dollars(usd)
            ));
        };
        if usd > max {
            return refuse(format!(
                "this operation costs {}, more than the {} this request allows; nothing ran",
                dollars(usd),
                dollars(max)
            ));
        }
        let mut item = operation.kind.clone();
        for path in &operation.paths {
            item.push(' ');
            item.push_str(path);
        }
        let item = item.chars().take(200).collect::<String>();
        let quote = Quote {
            charge: json!({ "item": item, "usd": usd }).to_string(),
            resumable: frame.get("resumable").and_then(Value::as_bool) == Some(true),
        };
        self.quotes.update(|quotes| {
            quotes.insert(operation.id, quote.clone());
        });
        Screened::Pass
    }

    pub(super) fn stamp(&self, text: String) -> String {
        if self.quotes.read(HashMap::is_empty) {
            return text;
        }
        let mut end = text.len().min(256);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let head = &text[..end];
        let reply = head.contains("\"type\":\"reply\"");
        if !reply && !head.contains("\"type\":\"update\"") {
            return text;
        }
        let Some(id) = head.find("\"id\":").and_then(|at| {
            let digits = &head[at + 5..];
            let length = digits.bytes().take_while(u8::is_ascii_digit).count();
            digits[..length].parse::<u64>().ok()
        }) else {
            return text;
        };
        let Some(quote) = self.quotes.read(|quotes| quotes.get(&id).cloned()) else {
            return text;
        };
        if reply {
            let failed = text
                .find("\"result\":")
                .is_some_and(|at| text[at + 9..].starts_with("{\"Err\""));
            if failed || !quote.resumable {
                self.quotes.update(|quotes| {
                    quotes.remove(&id);
                });
            }
            if failed {
                return text;
            }
        }
        append(text, "charge", &quote.charge)
    }
}

fn append(mut text: String, key: &str, value: &str) -> String {
    let Some(end) = text.rfind('}') else {
        return text;
    };
    text.truncate(end);
    text.push_str(",\"");
    text.push_str(key);
    text.push_str("\":");
    text.push_str(value);
    text.push('}');
    text
}

fn dollars(usd: f64) -> String {
    let text = format!("{usd:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("${text}")
}

const READING: &[&str] = &[
    "Glob",
    "Grep",
    "LS",
    "NotebookRead",
    "Read",
    "grep_files",
    "list_dir",
    "read_file",
    "view_image",
];

const RUNNING: &[&str] = &[
    "Bash",
    "BashOutput",
    "KillShell",
    "exec_command",
    "local_shell",
    "shell",
    "write_stdin",
];

const WRITING: &[&str] = &[
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "Write",
    "apply_patch",
    "write_file",
];

fn access(name: &str) -> &'static str {
    if READING.contains(&name) {
        return "read";
    }
    if WRITING.contains(&name) {
        return "write";
    }
    if RUNNING.contains(&name) {
        return "run";
    }
    "tool"
}

fn operation(frame: &Value, cwd: &str) -> Option<PricedOperation> {
    let id = frame.get("id")?.as_u64()?;
    let (variant, body) = frame.get("operation")?.as_object()?.iter().next()?;
    let session = body
        .pointer("/context/session_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let agent = frame
        .get("agent")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let max = frame.get("max_usd").and_then(Value::as_f64);
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
    match variant.as_str() {
        "Execute" | "Wait" => Some(PricedOperation {
            access: "cell",
            agent,
            command: None,
            id,
            input: body.get("input").cloned().unwrap_or(Value::Null),
            kind: if variant == "Wait" { "wait" } else { "execute" }.into(),
            max,
            paths: Vec::new(),
            session,
            source: text("source"),
        }),
        "Call" => {
            let kind = text("name")?;
            let input = body.get("input").cloned().unwrap_or(Value::Null);
            let command = ["cmd", "command"]
                .iter()
                .find_map(|key| input.get(key).and_then(Value::as_str))
                .map(str::to_owned);
            Some(PricedOperation {
                access: access(&kind),
                agent,
                paths: paths(&input, command.as_deref(), cwd),
                command,
                id,
                input,
                kind,
                max,
                session,
                source: None,
            })
        }
        _ => None,
    }
}

fn paths(input: &Value, command: Option<&str>, cwd: &str) -> Vec<String> {
    let mut found = Vec::new();
    for key in ["dir_path", "file_path", "notebook_path", "path"] {
        if let Some(path) = input.get(key).and_then(Value::as_str) {
            found.push(path.to_owned());
        }
    }
    if let Some(listed) = input.get("paths").and_then(Value::as_array) {
        found.extend(listed.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    let patch = [Some(input), input.get("input"), input.get("patch")]
        .into_iter()
        .flatten()
        .find_map(Value::as_str);
    if let Some(patch) = patch {
        for line in patch.lines() {
            for marker in [
                "*** Add File: ",
                "*** Delete File: ",
                "*** Update File: ",
                "*** Move to: ",
            ] {
                if let Some(path) = line.strip_prefix(marker) {
                    found.push(path.trim().to_owned());
                }
            }
        }
    }
    if let Some(command) = command {
        found.extend(words(command));
    }
    let base = ["workdir", "cwd"]
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str));
    let mut normalized = Vec::new();
    for path in found {
        let path = normalize(&path, base, cwd);
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    normalized
}

fn words(command: &str) -> Vec<String> {
    let mut found = Vec::new();
    for segment in command.split(['\n', ';', '|', '&', '(', ')']) {
        let mut first = true;
        for word in split(segment) {
            let word = word.trim_start_matches(|character: char| character.is_ascii_digit());
            let word = word.trim_start_matches(['<', '>']);
            let assignment = word.split_once('=').is_some_and(|(name, _)| {
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            });
            if first && assignment {
                continue;
            }
            if first {
                first = false;
                continue;
            }
            if !word.is_empty() && !word.starts_with('-') {
                found.push(word.to_owned());
            }
        }
    }
    found
}

fn split(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for character in segment.chars() {
        match (quote, character) {
            (Some(open), _) if character == open => quote = None,
            (Some(_), _) => word.push(character),
            (None, '\'' | '"') => {
                quote = Some(character);
                started = true;
            }
            (None, _) if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, _) => {
                word.push(character);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

fn normalize(path: &str, base: Option<&str>, cwd: &str) -> String {
    let from = match base {
        None => cwd.to_owned(),
        Some(base) if base.starts_with('/') => base.to_owned(),
        Some(base) => format!("{cwd}/{base}"),
    };
    let joined = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{from}/{path}")
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let absolute = format!("/{}", parts.join("/"));
    if absolute == cwd || (cwd.is_empty() && absolute == "/") {
        return ".".into();
    }
    absolute
        .strip_prefix(&format!("{cwd}/"))
        .map_or(absolute.clone(), str::to_owned)
}
