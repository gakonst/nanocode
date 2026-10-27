use std::collections::HashSet;

use nanocode_agent_core::{
    HarnessFamily, SessionCheckpoint,
    session::{ToolOutcome, TranscriptItem},
};
use nanocode_durability::{
    AdmittedTurn, CheckpointFormat, EncodedPayload, Error, FormatFuture, PayloadReader, Result,
    Sequence, SessionRecord, load_sequence,
};
use serde::Deserialize;
use serde_json::Value;

pub struct ClaudeCheckpointFormat;

#[derive(Deserialize)]
struct PagedConversation {
    version: u32,
    messages: Sequence,
    #[serde(default)]
    code_calls: Option<Sequence>,
    #[serde(default)]
    admitted_base: Option<EncodedPayload>,
    #[serde(default)]
    admitted_tool_ids: Vec<String>,
    #[serde(default)]
    unadmitted_tool_ids: HashSet<String>,
}

#[derive(Deserialize)]
struct AdmittedBase {
    ids: Vec<String>,
}

impl PagedConversation {
    async fn restore(
        self,
        reader: PayloadReader<'_>,
        conversation: &mut serde_json::Map<String, Value>,
        read: &mut HashSet<String>,
    ) -> Result<()> {
        let messages: Vec<Value> = load_sequence(reader, self.messages, read).await?;
        if let Some(code_calls) = self.code_calls {
            let rounds = load_sequence(reader, code_calls, read).await?;
            conversation.insert("code_calls".into(), Value::Array(rounds));
        }
        if self.version >= 3 {
            let mut admitted = match self.admitted_base {
                Some(base) => {
                    read.insert(base.key().to_owned());
                    reader
                        .load_payload(base)
                        .await?
                        .decode::<AdmittedBase>()?
                        .ids
                }
                None => Vec::new(),
            };
            admitted.extend(self.admitted_tool_ids);
            admitted.extend(
                messages
                    .iter()
                    .filter_map(|message| message["content"].as_array())
                    .flatten()
                    .filter(|block| block["type"] == "tool_use")
                    .filter_map(|block| block["id"].as_str())
                    .filter(|id| !self.unadmitted_tool_ids.contains(*id))
                    .map(str::to_owned),
            );
            conversation.insert(
                "admitted_tool_ids".into(),
                Value::Array(admitted.into_iter().map(Value::String).collect()),
            );
        }
        conversation.insert("messages".into(), Value::Array(messages));
        Ok(())
    }
}

#[derive(Deserialize)]
struct PagedSnapshot {
    #[serde(flatten)]
    conversation: PagedConversation,
    snapshot: Value,
}

#[derive(Deserialize)]
struct PagedCursor {
    #[serde(flatten)]
    conversation: PagedConversation,
    pending: Sequence,
    template: EncodedPayload,
    cursor: Value,
}

fn paged(value: &Value) -> bool {
    value
        .get("version")
        .and_then(Value::as_u64)
        .is_some_and(|version| (2..=4).contains(&version))
}

fn object<'a>(
    value: &'a mut Value,
    pointer: &str,
) -> Result<&'a mut serde_json::Map<String, Value>> {
    value
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::InvalidState(format!("Claude checkpoint has no {pointer}")))
}

fn insert(value: &mut Value, pointer: &str, field: &str, items: Vec<Value>) -> Result<()> {
    object(value, pointer)?.insert(field.into(), Value::Array(items));
    Ok(())
}

pub(super) async fn hydrate_snapshot(
    reader: PayloadReader<'_>,
    stored: Value,
    read: &mut HashSet<String>,
) -> Result<Value> {
    if !paged(&stored) {
        return Ok(stored);
    }
    let paged: PagedSnapshot = serde_json::from_value(stored).map_err(Error::InvalidPayload)?;
    let mut snapshot = paged.snapshot;
    paged
        .conversation
        .restore(reader, object(&mut snapshot, "/conversation")?, read)
        .await?;
    Ok(snapshot)
}

pub(super) async fn hydrate_cursor(reader: PayloadReader<'_>, stored: Value) -> Result<Value> {
    if !paged(&stored) {
        return Ok(stored);
    }
    let paged: PagedCursor = serde_json::from_value(stored).map_err(Error::InvalidPayload)?;
    let read = &mut HashSet::new();
    let pending = load_sequence(reader, paged.pending, read).await?;
    let prompt: serde_json::Map<String, Value> =
        reader.load_payload(paged.template).await?.decode()?;
    let mut cursor = paged.cursor;
    paged
        .conversation
        .restore(reader, object(&mut cursor, "/snapshot/conversation")?, read)
        .await?;
    insert(&mut cursor, "", "pending", pending)?;
    object(&mut cursor, "/template")?.extend(prompt);
    Ok(cursor)
}

async fn record(reader: PayloadReader<'_>, key: &str) -> Result<Value> {
    reader
        .load_payload(EncodedPayload::by_key(key))
        .await?
        .decode()
}

pub(super) async fn hydrate_receipt(
    reader: PayloadReader<'_>,
    mut receipt: Value,
) -> Result<Value> {
    let Some(Value::String(key)) = receipt
        .as_object_mut()
        .and_then(|fields| fields.remove("content_record"))
    else {
        return Ok(receipt);
    };
    let message = record(reader, &key).await?;
    let content = message.get("content").cloned().ok_or_else(|| {
        Error::InvalidState(format!("Claude message record {key} has no content"))
    })?;
    object(&mut receipt, "")?.insert("content".into(), content);
    Ok(receipt)
}

pub(super) async fn hydrate_output(reader: PayloadReader<'_>, mut output: Value) -> Result<Value> {
    let Some(Value::String(key)) = output
        .as_object_mut()
        .and_then(|fields| fields.remove("final_record"))
    else {
        return Ok(output);
    };
    let message = record(reader, &key).await?;
    let text = message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<String>();
    object(&mut output, "")?.insert("final_message".into(), Value::String(text));
    Ok(output)
}

impl CheckpointFormat for ClaudeCheckpointFormat {
    fn family(&self) -> HarnessFamily {
        HarnessFamily::Claude
    }

    fn owns(&self, checkpoint: &Value) -> bool {
        checkpoint.get("provider").and_then(Value::as_str) == Some("claude")
    }

    fn hydrate<'a>(
        &'a self,
        reader: PayloadReader<'a>,
        stored: Value,
    ) -> FormatFuture<'a, Result<Value>> {
        Box::pin(async move { hydrate_snapshot(reader, stored, &mut HashSet::new()).await })
    }

    fn hydrate_noting<'a>(
        &'a self,
        reader: PayloadReader<'a>,
        stored: Value,
        read: &'a mut HashSet<String>,
    ) -> FormatFuture<'a, Result<Value>> {
        Box::pin(hydrate_snapshot(reader, stored, read))
    }

    fn hydrate_continuation<'a>(
        &'a self,
        reader: PayloadReader<'a>,
        continuation: Value,
    ) -> FormatFuture<'a, Result<Option<Value>>> {
        Box::pin(async move {
            let mut continuation = hydrate_cursor(reader, continuation).await?;
            let Some(mut snapshot) = continuation
                .get_mut("snapshot")
                .map(Value::take)
                .filter(|snapshot| self.owns(snapshot))
            else {
                return Ok(None);
            };
            let base = continuation.get("pending_base").and_then(Value::as_u64);
            if let (Some(pending), Some(conversation)) = (
                continuation
                    .get_mut("pending")
                    .and_then(Value::as_array_mut),
                snapshot
                    .get_mut("conversation")
                    .and_then(Value::as_object_mut),
            ) {
                let mut pending = std::mem::take(pending);
                if let Some(base) = base {
                    let prefix = usize::try_from(base)
                        .ok()
                        .and_then(|base| {
                            conversation
                                .get("messages")
                                .and_then(Value::as_array)
                                .and_then(|messages| messages.get(..base))
                        })
                        .ok_or_else(|| {
                            Error::InvalidState(
                                "Claude continuation pending prefix exceeds its conversation"
                                    .to_owned(),
                            )
                        })?;
                    pending.splice(0..0, prefix.iter().cloned());
                }
                if !pending.is_empty() {
                    conversation.insert("messages".into(), Value::Array(pending));
                    conversation.insert("summary".into(), Value::String(String::new()));
                }
            }
            Ok(Some(snapshot))
        })
    }

    fn session_checkpoint(
        &self,
        record: &SessionRecord,
        checkpoint: Value,
    ) -> Result<SessionCheckpoint> {
        crate::session_checkpoint(&record.session_id, record.lineage.clone(), checkpoint)
            .map_err(|error| Error::InvalidState(error.to_string()))
    }

    fn transcript(&self, checkpoint: &Value, turns: &[AdmittedTurn<'_>]) -> Vec<TranscriptItem> {
        claude_transcript(checkpoint, &admitted_prompts(turns))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PromptPart {
    Text(String),
    Media,
}

struct AdmittedPrompt {
    parts: Vec<PromptPart>,
    steers: usize,
}

fn admitted_prompts(turns: &[AdmittedTurn<'_>]) -> Vec<AdmittedPrompt> {
    turns
        .iter()
        .filter_map(|turn| {
            let input = turn.input;
            if input["provider"] != "claude" || input["kind"] != "prompt" {
                return None;
            }
            let parts = match &input["prompt"]["instruction"] {
                Value::String(text) => vec![PromptPart::Text(text.clone())],
                Value::Array(items) => items
                    .iter()
                    .map(|item| match item["type"].as_str() {
                        Some("text") => {
                            PromptPart::Text(item["text"].as_str().unwrap_or_default().to_owned())
                        }
                        _ => PromptPart::Media,
                    })
                    .collect(),
                _ => return None,
            };
            Some(AdmittedPrompt {
                parts,
                steers: turn.steers,
            })
        })
        .collect()
}
fn blocks(message: &Value) -> impl Iterator<Item = &Value> {
    message["content"].as_array().into_iter().flatten()
}

fn prompt_parts(message: &Value) -> Vec<PromptPart> {
    blocks(message)
        .filter_map(|block| match block["type"].as_str()? {
            "text" => Some(PromptPart::Text(block["text"].as_str()?.to_owned())),
            "image" | "document" => Some(PromptPart::Media),
            _ => None,
        })
        .collect()
}

fn prompt_display(message: &Value) -> String {
    let (mut text, mut images, mut documents) = (String::new(), 0, 0);
    for block in blocks(message) {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
            Some("image") => {
                images += 1;
                text.push_str(&format!("[Image #{images}]"));
            }
            Some("document") => {
                documents += 1;
                text.push_str(&format!("[Document #{documents}]"));
            }
            _ => {}
        }
    }
    text
}

fn recovery_notice(message: &Value, notices: &[&str]) -> bool {
    let mut text = String::new();
    for block in blocks(message) {
        match block["text"].as_str() {
            Some(part) if block["type"] == "text" => text.push_str(part),
            _ => return false,
        }
    }
    notices.contains(&text.as_str())
}

fn tool_output(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item["type"].as_str()? {
                "text" => item["text"].as_str().map(str::to_owned),
                "image" => Some("[image]".to_owned()),
                "document" => Some("[document]".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn user_rows(messages: &[&Value], notices: &[&str], items: &mut Vec<TranscriptItem>) {
    for message in messages {
        if recovery_notice(message, notices) {
            continue;
        }
        let text = prompt_display(message);
        if !text.is_empty() {
            items.push(TranscriptItem::User(text));
        }
    }
}

fn steer_rows(
    segment: &[&Value],
    steers: usize,
    notices: &[&str],
    items: &mut Vec<TranscriptItem>,
) {
    user_rows(
        &segment[segment.len().saturating_sub(steers)..],
        notices,
        items,
    );
}

fn claude_assistant_rows(message: &Value, items: &mut Vec<TranscriptItem>) {
    for block in blocks(message) {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(text) = block["text"].as_str() {
                    items.push(TranscriptItem::Assistant(text.into()));
                }
            }

            Some("thinking") => {
                if let Some(text) = block["thinking"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                {
                    items.push(TranscriptItem::Reasoning(text.into()));
                }
            }
            Some("tool_use" | "server_tool_use") => items.push(TranscriptItem::Tool {
                call_id: block["id"].as_str().unwrap_or_default().into(),
                name: block["name"].as_str().unwrap_or_default().into(),
                arguments: block["input"].to_string(),
                parent_call_id: None,
            }),
            Some("mcp_tool_use") => items.push(TranscriptItem::Tool {
                call_id: block["id"].as_str().unwrap_or_default().into(),
                name: format!(
                    "mcp__{}__{}",
                    block["server_name"].as_str().unwrap_or_default(),
                    block["name"].as_str().unwrap_or_default()
                ),
                arguments: block["input"].to_string(),
                parent_call_id: None,
            }),
            _ => {}
        }
    }
}

fn claude_transcript(checkpoint: &Value, prompts: &[AdmittedPrompt]) -> Vec<TranscriptItem> {
    let mut items = Vec::new();
    let conversation = &checkpoint["conversation"];
    if let Some(summary) = conversation["summary"]
        .as_str()
        .filter(|summary| !summary.is_empty())
    {
        items.push(TranscriptItem::Assistant(format!(
            "Retained conversation summary:\n{summary}"
        )));
    }
    let notices = conversation["recovery_notices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let messages = conversation["messages"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let receipt = |message: &Value| blocks(message).any(|block| block["type"] == "tool_result");

    let mut children = std::collections::HashMap::<&str, Vec<(&str, &Value)>>::new();
    let mut outcomes = std::collections::HashMap::<&str, &str>::new();
    for round in conversation["code_calls"].as_array().into_iter().flatten() {
        let Some(receipt_id) = round["tool_use_id"].as_str() else {
            continue;
        };
        let origin = round["origin_call_id"].as_str();
        for call in round["calls"].as_array().into_iter().flatten() {
            if let Some(call_id) = call["call_id"].as_str() {
                outcomes.insert(call_id, call["status"].as_str().unwrap_or("unknown"));
                let cell = call["parent_call_id"]
                    .as_str()
                    .or(origin)
                    .unwrap_or(receipt_id);
                children.entry(receipt_id).or_default().push((cell, call));
            }
        }
    }
    let mut replayed_children = std::collections::HashSet::new();
    let mut next_prompt = 0;
    let mut index = 0;
    while let Some(message) = messages.get(index) {
        if message["role"].as_str() == Some("assistant") {
            claude_assistant_rows(message, &mut items);
            index += 1;
            continue;
        }
        if receipt(message) {
            for block in blocks(message).filter(|block| block["type"] == "tool_result") {
                let parent = block["tool_use_id"].as_str().unwrap_or_default();
                for (cell, call) in children.get(parent).into_iter().flatten() {
                    let call_id = call["call_id"].as_str().unwrap_or_default();
                    if !replayed_children.insert(call_id) {
                        continue;
                    }
                    items.push(TranscriptItem::Tool {
                        call_id: call_id.into(),
                        name: call["name"].as_str().unwrap_or_default().into(),
                        arguments: call["input"].to_string(),
                        parent_call_id: Some((*cell).into()),
                    });
                    let outcome = match outcomes.get(call_id).copied() {
                        Some("completed") => ToolOutcome::Completed,
                        Some("failed") => ToolOutcome::Failed,
                        _ => ToolOutcome::Unknown,
                    };
                    items.push(TranscriptItem::tool_result(call_id, "", outcome));
                }
                items.push(TranscriptItem::tool_result(
                    parent,
                    &tool_output(&block["content"]),
                    if block["is_error"].as_bool() == Some(true) {
                        ToolOutcome::Failed
                    } else {
                        ToolOutcome::Completed
                    },
                ));
            }

            let content = blocks(message)
                .filter(|block| block["type"] != "tool_result")
                .cloned()
                .collect::<Vec<_>>();
            if !content.is_empty() {
                let shared = serde_json::json!({ "content": content });
                user_rows(&[&shared], &notices, &mut items);
            }
            index += 1;
            continue;
        }

        let end = messages[index..]
            .iter()
            .position(|message| message["role"].as_str() != Some("user") || receipt(message))
            .map_or(messages.len(), |offset| index + offset);
        let run = &messages[index..end];
        index = end;
        let mut segment = Vec::new();
        let mut steers = None;
        for message in run {
            let parts = prompt_parts(message);
            let Some(offset) = prompts
                .get(next_prompt..)
                .and_then(|rest| rest.iter().position(|prompt| prompt.parts == parts))
            else {
                segment.push(message);
                continue;
            };
            match steers {
                Some(steers) => steer_rows(&segment, steers, &notices, &mut items),

                None if offset == 0 => {}
                None => user_rows(&segment, &notices, &mut items),
            }
            segment.clear();
            steers = Some(prompts[next_prompt + offset].steers);
            next_prompt += offset + 1;
            items.push(TranscriptItem::User(prompt_display(message)));
        }

        match steers {
            Some(steers) => steer_rows(&segment, steers, &notices, &mut items),
            None => user_rows(&segment, &notices, &mut items),
        }
    }
    items
}
