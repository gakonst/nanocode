use std::time::Duration;

use opentelemetry::{KeyValue, Value};
use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    trace::{SpanData, SpanExporter},
};
use serde_json::{Value as Json, json};

#[derive(Clone, Copy, Debug)]
pub struct Agent {
    pub span: &'static str,
    pub name: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exported {
    Sent(u64),
    Failed(u64),
    Unattributed,
}

#[derive(Debug)]
pub struct GenAi<E> {
    exporter: E,
    agent: Option<Agent>,
    exported: fn(Exported),
}

impl<E> GenAi<E> {
    pub const fn new(exporter: E, agent: Option<Agent>, exported: fn(Exported)) -> Self {
        Self {
            exporter,
            agent,
            exported,
        }
    }
}

impl<E: SpanExporter> SpanExporter for GenAi<E> {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let count = u64::try_from(batch.len()).unwrap_or(u64::MAX);
        let batch = batch
            .into_iter()
            .map(|span| gen_ai(span, self.agent, self.exported))
            .collect();
        let exported = self.exporter.export(batch).await;
        (self.exported)(match exported {
            Ok(()) => Exported::Sent(count),
            Err(_) => Exported::Failed(count),
        });
        exported
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.exporter.shutdown_with_timeout(timeout)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.exporter.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.exporter.set_resource(resource);
    }
}

enum Operation {
    Chat,
    ExecuteTool,
    InvokeAgent(&'static str),
}

impl Operation {
    fn of(span: &str, agent: Option<Agent>) -> Option<Self> {
        match span {
            "model.call" => Some(Self::Chat),
            "tool.call" | "tool.execute" => Some(Self::ExecuteTool),
            span => agent
                .filter(|agent| agent.span == span)
                .map(|agent| Self::InvokeAgent(agent.name)),
        }
    }

    const fn name(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::ExecuteTool => "execute_tool",
            Self::InvokeAgent(_) => "invoke_agent",
        }
    }

    const fn renames(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Chat => &[
                ("model", "gen_ai.request.model"),
                ("model.response.id", "gen_ai.response.id"),
                ("input_tokens", "gen_ai.usage.input_tokens"),
                ("output_tokens", "gen_ai.usage.output_tokens"),
            ],
            Self::ExecuteTool => &[
                ("tool.name", "gen_ai.tool.name"),
                ("tool.call_id", "gen_ai.tool.call.id"),
            ],
            Self::InvokeAgent(_) => &[],
        }
    }

    fn subject(&self, span: &SpanData) -> Option<Value> {
        match self {
            Self::Chat => attribute(span, "model"),
            Self::ExecuteTool => attribute(span, "tool.name"),
            Self::InvokeAgent(name) => Some(Value::from(*name)),
        }
    }
}

fn attribute(span: &SpanData, key: &str) -> Option<Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.clone())
}

fn gen_ai(mut span: SpanData, agent: Option<Agent>, exported: fn(Exported)) -> SpanData {
    content(&mut span);
    let Some(operation) = Operation::of(span.name.as_ref(), agent) else {
        return span;
    };
    let mut added = vec![KeyValue::new("gen_ai.operation.name", operation.name())];
    added.extend(
        operation
            .renames()
            .iter()
            .filter_map(|(from, to)| Some(KeyValue::new(*to, attribute(&span, from)?))),
    );
    if let Some(subject) = operation.subject(&span) {
        span.name = format!("{} {subject}", operation.name()).into();
    }
    if let Operation::Chat = operation {
        if let Some(provider) = model_provider(&span) {
            span.attributes
                .retain(|kv| !matches!(kv.key.as_str(), "gen_ai.provider.name" | "gen_ai.system"));
            added.push(KeyValue::new("gen_ai.provider.name", provider.clone()));
            added.push(KeyValue::new("gen_ai.system", provider));
        } else {
            exported(Exported::Unattributed);
        }
    }
    span.attributes.extend(added);
    span
}

fn model_provider(span: &SpanData) -> Option<Value> {
    for key in ["gen_ai.provider.name", "gen_ai.system", "provider"] {
        if let Some(value) = attribute(span, key) {
            return Some(value);
        }
    }
    for key in ["code.module.name", "target"] {
        if let Some(Value::String(value)) = attribute(span, key) {
            match value.as_str().split("::").next() {
                Some("nanocode_backend_claude" | "nanocode_claude") => {
                    return Some(Value::from("anthropic"));
                }
                Some("nanocode_backend_codex" | "nanocode_codex") => {
                    return Some(Value::from("openai"));
                }
                _ => {}
            }
        }
    }
    None
}

fn content(span: &mut SpanData) {
    let (mut input, mut output) = (Vec::new(), Vec::new());
    let mut added = Vec::new();
    span.events.events.retain(|event| {
        let get = |key: &str| {
            event
                .attributes
                .iter()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.as_str().into_owned())
        };
        let (Some(kind), Some(content)) = (get("content_kind"), get("content")) else {
            return true;
        };
        match kind.as_str() {
            "prompt" => input.extend(prompt(&content)),
            "model.input" => input.extend(items(&content).iter().map(message)),
            "model.output_item" | "reasoning" => output.extend(
                items(&format!("[{content}]"))
                    .iter()
                    .flat_map(|item| parts(message(item))),
            ),
            "tool.arguments" => added.push(KeyValue::new("gen_ai.tool.call.arguments", content)),
            "tool.output" | "tool.panic" => {
                added.push(KeyValue::new("gen_ai.tool.call.result", content));
            }
            _ => return true,
        }
        false
    });
    if !input.is_empty() {
        added.push(KeyValue::new(
            "gen_ai.input.messages",
            Json::from(input).to_string(),
        ));
    }
    if !output.is_empty() {
        let called = output.iter().any(|part| part["type"] == "tool_call");
        let finish = if called { "tool_call" } else { "stop" };
        let message = json!([{"role": "assistant", "parts": output, "finish_reason": finish}]);
        added.push(KeyValue::new("gen_ai.output.messages", message.to_string()));
    }
    span.attributes.extend(added);
}

fn items(content: &str) -> Vec<Json> {
    serde_json::from_str(content).unwrap_or_default()
}

fn parts(message: Json) -> Vec<Json> {
    match message {
        Json::Object(mut message) => match message.remove("parts") {
            Some(Json::Array(parts)) => parts,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn text(content: &Json) -> Json {
    json!({"type": "text", "content": content})
}

fn prompt(content: &str) -> Vec<Json> {
    let user = |parts| json!({"role": "user", "parts": parts});
    let Ok(Json::Object(prompt)) = serde_json::from_str::<Json>(content) else {
        return vec![user(vec![text(&Json::from(content))])];
    };
    let Some(instruction) = prompt.get("instruction") else {
        return vec![user(vec![text(&Json::from(content))])];
    };
    let mut messages: Vec<Json> = prompt
        .get("transcript")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .map(|message| json!({"role": message["role"], "parts": [text(&message["content"])]}))
        .collect();
    messages.push(user(match instruction {
        Json::Array(items) => items
            .iter()
            .map(|item| item.get("text").map_or_else(|| item.clone(), text))
            .collect(),
        instruction => vec![text(instruction)],
    }));
    messages
}

fn message(item: &Json) -> Json {
    let call = |key: &str| {
        let value = &item[key];
        value
            .as_str()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_else(|| value.clone())
    };
    match item["type"].as_str().unwrap_or("message") {
        "message" => {
            let parts: Vec<Json> = match &item["content"] {
                Json::Array(parts) => parts
                    .iter()
                    .map(|part| part.get("text").map_or_else(|| part.clone(), text))
                    .collect(),
                content => vec![text(content)],
            };
            json!({"role": item["role"], "parts": parts})
        }
        "function_call" | "custom_tool_call" => {
            let arguments = if item.get("arguments").is_some() {
                call("arguments")
            } else {
                call("input")
            };
            json!({"role": "assistant", "parts": [{
                "type": "tool_call", "id": item["call_id"], "name": item["name"], "arguments": arguments,
            }]})
        }
        "function_call_output" | "custom_tool_call_output" => json!({"role": "tool", "parts": [{
            "type": "tool_call_response", "id": item["call_id"], "response": call("output"),
        }]}),
        "reasoning" => {
            let summary = item["summary"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            json!({"role": "assistant", "parts": [{"type": "reasoning", "content": summary}]})
        }
        _ => {
            let role = item
                .get("role")
                .cloned()
                .unwrap_or_else(|| "assistant".into());
            json!({"role": role, "parts": [item]})
        }
    }
}
