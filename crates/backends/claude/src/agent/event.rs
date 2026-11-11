use super::telemetry::CallTiming;
use crate::{CacheTtl, StopReason, ToolResultContent};
use nanocode_agent_core::{CostStatus, EstimatedUsdCost, input::PromptInput};
use nanocode_types::preview;
use serde::{Serialize, Serializer, ser::SerializeMap as _};
use serde_json::Value;
use std::borrow::Cow;

#[derive(Serialize)]
pub(super) struct InTurn<'a, T> {
    #[serde(flatten)]
    pub(super) payload: T,
    pub(super) turn_id: &'a str,
}

#[derive(Serialize)]
pub(super) struct InputAccepted<'a> {
    pub(super) session_id: &'a str,
    pub(super) turn_id: &'a str,
    pub(super) item_id: String,
    pub(super) kind: &'static str,
    pub(super) request_id: Option<&'a str>,
    pub(super) input: &'a PromptInput,
}

#[derive(Serialize)]
pub(super) struct RunStarted<'a> {
    pub(super) mode: &'static str,
    pub(super) model: &'a str,
    pub(super) reasoning_mode: &'a str,
    pub(super) effort: &'a str,
    pub(super) transport: &'static str,
    pub(super) orchestration: &'static str,
    pub(super) websocket_url: &'static str,
    pub(super) workspace: &'a str,
    pub(super) instruction_bytes: usize,
}

#[derive(Serialize)]
pub(super) struct RunFinished<'a> {
    pub(super) status: &'static str,
    pub(super) model: &'a str,
    pub(super) reasoning_mode: &'a str,
    pub(super) effort: &'a str,
    pub(super) transport: &'static str,
    pub(super) orchestration: &'static str,
    pub(super) duration_ms: u64,
    pub(super) duration_ns: u64,
    pub(super) estimated_cost: Option<&'a EstimatedUsdCost>,
    pub(super) cost_usd: Option<f64>,
    pub(super) cost_status: CostStatus,
}

#[derive(Serialize)]
pub(super) struct ModelCallStarted<'a> {
    pub(super) call_index: u32,
    pub(super) model: &'a str,
    pub(super) reasoning_mode: &'a str,
    pub(super) effort: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cache_ttl: Option<CacheTtl>,
}

#[derive(Serialize)]
pub(super) struct ModelCallCompleted<'a> {
    pub(super) call_index: u32,
    pub(super) model: &'a str,
    pub(super) response_id: &'a str,
    pub(super) attempt: u32,
    pub(super) connection_generation: u32,
    pub(super) status: StopReason,
    pub(super) duration_ns: u64,
    pub(super) time_to_first_event_ns: u64,
    pub(super) time_to_first_output_ns: Option<u64>,
    pub(super) time_to_dispatch_ns: Option<u64>,
    pub(super) tool_calls: usize,
    pub(super) usage: ModelCallUsage,
    pub(super) timing: CallTiming,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) message_thread: Option<MessageThread>,
}

#[derive(Serialize)]
pub(super) struct ModelCallUsage {
    pub(super) input_tokens: u64,
    pub(super) input_tokens_details: InputTokensDetails,
    pub(super) output_tokens: u64,
    pub(super) total_tokens: u64,
}

#[derive(Serialize)]
pub(super) struct InputTokensDetails {
    pub(super) cached_tokens: u64,
    pub(super) cache_write_tokens: u64,
}

#[derive(Serialize)]
pub(super) struct MessageThread {
    pub(super) mode: &'static str,
    pub(super) create_reason: Option<&'static str>,
}

#[derive(Serialize)]
pub(super) struct AssistantMessage<'a> {
    pub(super) model_call_index: u32,
    pub(super) item_id: &'a str,
    pub(super) phase: Option<&'a str>,
    pub(super) text: &'a str,
    pub(super) citations: &'a [Value],
}

#[derive(Serialize)]
pub(super) struct ToolCall<'a> {
    pub(super) call_id: &'a str,
    pub(super) tool: &'a str,
    pub(super) arguments: Cow<'a, Value>,
    pub(super) model_call_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parent_call_id: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) original_bytes: Option<usize>,
}

impl<'a> ToolCall<'a> {
    pub(super) fn nested(
        call_id: &'a str,
        tool: &'a str,
        input: Option<&'a Value>,
        model_call_index: u32,
        parent: &'a Value,
    ) -> Self {
        let (arguments, original_bytes) = match input {
            None => (Cow::Owned(Value::Null), None),
            Some(input) => match preview::shortened(input) {
                Some((arguments, size)) => (Cow::Owned(arguments), Some(size)),
                None => (Cow::Borrowed(input), None),
            },
        };
        Self {
            call_id,
            tool,
            arguments,
            model_call_index,
            parent_call_id: Some(parent),
            truncated: original_bytes.map(|_| true),
            original_bytes,
        }
    }
}

#[derive(Serialize)]
struct Text<T> {
    text: T,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Shown<'a> {
    Text(&'a str),
    ContentBlocks(&'a [Value]),
}

#[derive(Serialize)]
struct CodeSummary<'a> {
    origin_call_id: Option<&'a Value>,
    nested_call_count: usize,
    running: Option<&'a Value>,
}

struct Metadata<'a>(Option<&'a Value>);

impl Serialize for Metadata<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Some(Value::Object(object)) if object.contains_key("_nanocode_code") => {
                let mut map = serializer.serialize_map(Some(object.len()))?;
                for (key, value) in object {
                    let calls = value
                        .get("calls")
                        .and_then(Value::as_array)
                        .filter(|_| key == "_nanocode_code");
                    match calls {
                        Some(calls) => map.serialize_entry(
                            key,
                            &CodeSummary {
                                origin_call_id: value.get("origin_call_id"),
                                nested_call_count: calls.len(),
                                running: value.get("running"),
                            },
                        )?,
                        None => map.serialize_entry(key, value)?,
                    }
                }
                map.end()
            }
            Some(value) if preview::encoded_len(value) > preview::RESULT_BYTES => {
                serializer.serialize_unit()
            }
            value => value.serialize(serializer),
        }
    }
}

fn encoded_len(value: Option<&Value>) -> usize {
    value.map_or(0, preview::encoded_len)
}

fn image_reference(value: Option<&Value>) -> Value {
    value.map_or(Value::Null, preview::image_reference)
}

#[derive(Serialize)]
pub(super) struct ToolResult<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_bytes: Option<usize>,
    result: Shown<'a>,
    structured_result: Cow<'a, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_result_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_result_bytes: Option<usize>,
    metadata: Metadata<'a>,
    call_id: &'a str,
    tool: &'a str,
    status: &'static str,
    duration_ns: u64,
    started_after_ns: Option<u64>,
}

pub(super) struct Call<'a> {
    pub(super) call_id: &'a str,
    pub(super) tool: &'a str,
    pub(super) status: &'static str,
    pub(super) duration_ns: u64,
}

impl<'a> ToolResult<'a> {
    pub(super) fn new(
        call: Call<'a>,
        content: &'a ToolResultContent,
        original_bytes: Option<usize>,
        structured_result: Option<&'a Value>,
        metadata: Option<&'a Value>,
    ) -> Self {
        let structured_len = encoded_len(structured_result);
        let large = structured_len > preview::RESULT_BYTES;
        Self {
            truncated: original_bytes.map(|_| true),
            original_bytes,
            result: match content {
                ToolResultContent::Text(text) => Shown::Text(text),
                ToolResultContent::Blocks(blocks) => Shown::ContentBlocks(blocks),
            },
            structured_result: if large {
                Cow::Owned(image_reference(structured_result))
            } else {
                structured_result.map_or(Cow::Owned(Value::Null), Cow::Borrowed)
            },
            structured_result_truncated: large.then_some(true),
            structured_result_bytes: large.then_some(structured_len),
            metadata: Metadata(metadata),
            call_id: call.call_id,
            tool: call.tool,
            status: call.status,
            duration_ns: call.duration_ns,
            started_after_ns: None,
        }
    }
}

#[derive(Serialize)]
pub(super) struct NestedToolResult<'a> {
    result: Text<Cow<'a, Value>>,
    structured_result: Cow<'a, Value>,
    metadata: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_bytes: Option<Cow<'a, Value>>,
    call_id: &'a str,
    tool: &'a str,
    status: &'static str,
    duration_ns: Option<&'a Value>,
    started_after_ns: Option<&'a Value>,
    parent_call_id: &'a Value,
}

impl<'a> NestedToolResult<'a> {
    pub(super) fn new(call_id: &'a str, tool: &'a str, call: &'a Value, parent: &'a Value) -> Self {
        let output = call.get("output");
        let structured = call.get("structured_result");
        let metadata = call.get("metadata");
        let host_truncated = call.get("event_truncated").and_then(Value::as_bool) == Some(true);
        let duplicate = structured.is_some_and(|value| {
            output.and_then(Value::as_str).is_some_and(|text| {
                text.len() == encoded_len(Some(value))
                    && serde_json::from_str::<Value>(text).ok().as_ref() == Some(value)
            })
        });
        let size = encoded_len(output)
            + if duplicate {
                0
            } else {
                encoded_len(structured)
            }
            + encoded_len(metadata);
        let whole = host_truncated || size <= preview::RESULT_BYTES;
        let borrowed =
            |value: Option<&'a Value>| value.map_or(Cow::Owned(Value::Null), Cow::Borrowed);
        Self {
            result: Text {
                text: if whole {
                    borrowed(output)
                } else {
                    let text = match output {
                        Some(Value::String(text)) => Cow::Borrowed(text.as_str()),
                        Some(value) if !value.is_null() => Cow::Owned(value.to_string()),
                        _ => Cow::Owned(structured.map(Value::to_string).unwrap_or_default()),
                    };
                    Cow::Owned(Value::String(
                        preview::utf8_prefix(&text, preview::PREVIEW_BYTES).to_owned(),
                    ))
                },
            },
            structured_result: if whole {
                borrowed(structured)
            } else {
                Cow::Owned(image_reference(structured))
            },
            metadata: metadata
                .filter(|_| whole || encoded_len(metadata) <= preview::METADATA_BYTES),
            truncated: (host_truncated || !whole).then_some(true),
            original_bytes: if host_truncated {
                Some(borrowed(call.get("event_original_bytes")))
            } else if whole {
                None
            } else {
                Some(Cow::Owned(Value::from(size)))
            },
            call_id,
            tool,
            status: if call.get("success").and_then(Value::as_bool) == Some(true) {
                "completed"
            } else {
                "failed"
            },
            duration_ns: call.get("duration_ns"),
            started_after_ns: call.get("started_after_ns"),
            parent_call_id: parent,
        }
    }
}
