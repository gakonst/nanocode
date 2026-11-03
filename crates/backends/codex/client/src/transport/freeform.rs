use std::collections::HashSet;

use serde_json::{Value, json};

use crate::client::responses::{ResponseItem, ToolDefinition};

pub(crate) const CUSTOM_TOOL_HINT: &str = "Pass the exact free-form tool input as the JSON string field input. Preserve all code and newlines.";

pub(crate) fn input_parameters() -> Value {
    json!({
        "type": "object",
        "properties": { "input": { "type": "string" } },
        "required": ["input"],
        "additionalProperties": false,
    })
}

pub(crate) fn grammar_hint(syntax: &str, definition: &str) -> String {
    format!("Input grammar ({syntax}): {definition}")
}

pub(crate) fn function_item(item: &ResponseItem) -> Option<ResponseItem> {
    match item {
        ResponseItem::AdditionalTools { id, role, tools } if tools.iter().any(declares_custom) => {
            Some(ResponseItem::AdditionalTools {
                id: id.clone(),
                role: *role,
                tools: tools.iter().map(function_tool).collect(),
            })
        }
        ResponseItem::CustomToolCall {
            id,
            status,
            call_id,
            name,
            namespace,
            input,
            asynchronous,
            caller,
            created_by,
            internal_chat_message_metadata_passthrough,
        } => Some(ResponseItem::FunctionCall {
            id: id.clone(),
            name: name.clone(),
            namespace: namespace.clone(),
            arguments: json!({ "input": input }).to_string().into_boxed_str(),
            asynchronous: *asynchronous,
            encrypted_function_args: None,
            call_id: call_id.clone(),
            caller: caller.clone(),
            status: *status,
            created_by: created_by.clone(),
            internal_chat_message_metadata_passthrough: internal_chat_message_metadata_passthrough
                .clone(),
        }),
        ResponseItem::CustomToolCallOutput {
            id,
            call_id,
            output,
            caller,
            status,
            created_by,
            internal_chat_message_metadata_passthrough,
            ..
        } => Some(ResponseItem::FunctionCallOutput {
            id: id.clone(),
            call_id: call_id.clone(),
            output: output.clone(),
            caller: caller.clone(),
            status: *status,
            created_by: created_by.clone(),
            internal_chat_message_metadata_passthrough: internal_chat_message_metadata_passthrough
                .clone(),
        }),
        _ => None,
    }
}

fn declares_custom(tool: &ToolDefinition) -> bool {
    match tool {
        ToolDefinition::Custom { .. } => true,
        ToolDefinition::Namespace { tools, .. } => tools.iter().any(declares_custom),
        ToolDefinition::Function { .. } | ToolDefinition::ToolSearch { .. } => false,
    }
}

fn function_tool(tool: &ToolDefinition) -> ToolDefinition {
    match tool {
        ToolDefinition::Custom {
            name,
            description,
            defer_loading,
            asynchronous,
            format,
        } => {
            let mut lines = Vec::with_capacity(3);
            if !description.is_empty() {
                lines.push(description.to_string());
            }
            lines.push(CUSTOM_TOOL_HINT.to_owned());
            if !format.definition.is_empty() {
                lines.push(grammar_hint(&format.syntax, &format.definition));
            }
            ToolDefinition::Function {
                name: name.clone(),
                description: lines.join("\n").into(),
                strict: false,
                defer_loading: *defer_loading,
                asynchronous: *asynchronous,
                parameters: input_parameters().into(),
                output_schema: None,
            }
        }
        ToolDefinition::Namespace {
            name,
            description,
            tools,
        } => ToolDefinition::Namespace {
            name: name.clone(),
            description: description.clone(),
            tools: tools.iter().map(function_tool).collect(),
        },
        ToolDefinition::Function { .. } | ToolDefinition::ToolSearch { .. } => tool.clone(),
    }
}

pub(crate) struct FreeformCalls {
    tools: HashSet<(Option<String>, String)>,
    calls: Vec<StreamedCall>,
}

struct StreamedCall {
    item_id: Option<String>,
    output_index: Option<u64>,
    call_id: Value,
    arguments: String,
    input: InputScan,
}

enum InputScan {
    Seeking,
    Reading(usize),
    Closed,
}

impl FreeformCalls {
    pub(crate) fn new<'a>(items: impl IntoIterator<Item = &'a ResponseItem>) -> Self {
        fn collect(
            tools: &mut HashSet<(Option<String>, String)>,
            namespace: Option<&str>,
            tool: &ToolDefinition,
        ) {
            match tool {
                ToolDefinition::Custom { name, .. } => {
                    tools.insert((namespace.map(str::to_owned), name.to_string()));
                }
                ToolDefinition::Namespace {
                    name,
                    tools: children,
                    ..
                } => {
                    for child in children {
                        collect(tools, Some(name), child);
                    }
                }
                ToolDefinition::Function { .. } | ToolDefinition::ToolSearch { .. } => {}
            }
        }
        let mut tools = HashSet::new();
        for item in items {
            if let ResponseItem::AdditionalTools {
                tools: declared, ..
            } = item
            {
                for tool in declared {
                    collect(&mut tools, None, tool);
                }
            }
        }
        Self {
            tools,
            calls: Vec::new(),
        }
    }

    pub(crate) fn rewrite(&mut self, text: &str) -> Option<String> {
        if self.tools.is_empty() || !text.contains("function_call") {
            return None;
        }
        let mut event: Value = serde_json::from_str(text).ok()?;
        let kind = event.get("type")?.as_str()?.to_owned();
        match kind.as_str() {
            "response.output_item.added" => {
                let output_index = event.get("output_index").and_then(Value::as_u64);
                let item = event.get_mut("item")?;
                if !self.declared(item) {
                    return None;
                }
                let mut call = StreamedCall {
                    item_id: item.get("id").and_then(Value::as_str).map(str::to_owned),
                    output_index,
                    call_id: item.get("call_id").cloned().unwrap_or(Value::Null),
                    arguments: String::new(),
                    input: InputScan::Seeking,
                };
                let input = call.push(
                    item.get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
                custom_call(item, input);
                self.calls.push(call);
            }
            "response.output_item.done" => {
                let item = event.get_mut("item")?;
                if !self.declared(item) {
                    return None;
                }
                let input = final_input(item.get("arguments").and_then(Value::as_str)?)?;
                custom_call(item, input);
            }
            "response.function_call_arguments.delta" => {
                let call = self.streamed(&event)?;
                let delta = call.push(event.get("delta").and_then(Value::as_str)?);
                let call_id = call.call_id.clone();
                let event = event.as_object_mut()?;
                event.insert(
                    "type".to_owned(),
                    json!("response.custom_tool_call_input.delta"),
                );
                event.insert("call_id".to_owned(), call_id);
                event.insert("delta".to_owned(), Value::String(delta));
            }
            "response.function_call_arguments.done" => {
                let call = self.streamed(&event)?;
                let call_id = call.call_id.clone();
                let input = final_input(event.get("arguments").and_then(Value::as_str)?)?;
                let event = event.as_object_mut()?;
                event.insert(
                    "type".to_owned(),
                    json!("response.custom_tool_call_input.done"),
                );
                event.insert("call_id".to_owned(), call_id);
                event.remove("arguments");
                event.insert("input".to_owned(), Value::String(input));
            }
            "response.completed" => {
                let mut changed = false;
                for item in event
                    .get_mut("response")
                    .and_then(|response| response.get_mut("output"))
                    .and_then(Value::as_array_mut)?
                {
                    if self.declared(item)
                        && let Some(input) = item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .and_then(final_input)
                    {
                        custom_call(item, input);
                        changed = true;
                    }
                }
                if !changed {
                    return None;
                }
            }
            _ => return None,
        }
        Some(event.to_string())
    }

    fn declared(&self, item: &Value) -> bool {
        item.get("type").and_then(Value::as_str) == Some("function_call")
            && item
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| {
                    self.tools.contains(&(
                        item.get("namespace")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        name.to_owned(),
                    ))
                })
    }

    fn streamed(&mut self, event: &Value) -> Option<&mut StreamedCall> {
        let item_id = event.get("item_id").and_then(Value::as_str);
        let output_index = event.get("output_index").and_then(Value::as_u64);
        self.calls
            .iter_mut()
            .find(|call| match (item_id, call.item_id.as_deref()) {
                (Some(item_id), Some(id)) => item_id == id,
                _ => output_index.is_some() && output_index == call.output_index,
            })
    }
}

impl StreamedCall {
    fn push(&mut self, fragment: &str) -> String {
        self.arguments.push_str(fragment);
        let mut decoded = String::new();
        let mut at = match self.input {
            InputScan::Closed => return decoded,
            InputScan::Reading(at) => at,
            InputScan::Seeking => match input_start(&self.arguments) {
                Some(at) => at,
                None => return decoded,
            },
        };
        while let Some(character) = self.arguments[at..].chars().next() {
            match character {
                '"' => {
                    self.input = InputScan::Closed;
                    return decoded;
                }
                '\\' => {
                    let Some((text, length)) = escape(&self.arguments[at..]) else {
                        break;
                    };
                    decoded.push_str(&text);
                    at += length;
                }
                character => {
                    decoded.push(character);
                    at += character.len_utf8();
                }
            }
        }
        self.input = InputScan::Reading(at);
        decoded
    }
}

fn input_start(arguments: &str) -> Option<usize> {
    let key = arguments.find("\"input\"")? + "\"input\"".len();
    let rest = arguments[key..].trim_start().strip_prefix(':')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    Some(arguments.len() - rest.len())
}

fn escape(text: &str) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let length = match bytes.get(1)? {
        b'u' => {
            let unit = u16::from_str_radix(text.get(2..6)?, 16).ok()?;
            if (0xD800..0xDC00).contains(&unit) {
                12
            } else {
                6
            }
        }
        _ => 2,
    };
    let escaped = text.get(..length)?;
    let decoded = serde_json::from_str::<String>(&format!("\"{escaped}\"")).ok()?;
    Some((decoded, length))
}

fn final_input(arguments: &str) -> Option<String> {
    match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(mut arguments)) => match arguments.remove("input") {
            Some(Value::String(input)) => Some(input),
            _ => None,
        },
        Ok(_) => None,
        Err(_) => {
            let trimmed = arguments.trim_start();
            (!trimmed.is_empty() && !trimmed.starts_with(['{', '[', '"']))
                .then(|| arguments.to_owned())
        }
    }
}

fn custom_call(item: &mut Value, input: String) {
    if let Some(item) = item.as_object_mut() {
        item.insert("type".to_owned(), json!("custom_tool_call"));
        item.remove("arguments");
        item.remove("encrypted_function_args");
        item.insert("input".to_owned(), Value::String(input));
    }
}
