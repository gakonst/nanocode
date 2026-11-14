use serde_json::{Map, Value, json};

const CODE_FIELD: &str = "code";
const INPUT_FIELD: &str = "input";
const ASSISTANT: &str = "assistant";
const USER: &str = "user";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolKind {
    Function,
    Custom,
    CustomAsFunction,
}

pub struct Tools(Vec<(String, ToolKind)>);

fn takes_only_input(parameters: Option<&Value>) -> bool {
    parameters
        .and_then(|parameters| parameters.get("properties"))
        .and_then(Value::as_object)
        .is_some_and(|properties| properties.len() == 1 && properties.contains_key(INPUT_FIELD))
}

impl Tools {
    pub fn of(request: &Value) -> Self {
        let declared = request
            .get("tools")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        Self(
            declared
                .iter()
                .filter_map(|tool| {
                    let name = tool.get("name").and_then(Value::as_str)?;
                    let kind = match tool.get("type").and_then(Value::as_str)? {
                        "custom" => ToolKind::Custom,
                        "function" if takes_only_input(tool.get("parameters")) => {
                            ToolKind::CustomAsFunction
                        }
                        "function" => ToolKind::Function,
                        _ => return None,
                    };
                    Some((name.to_owned(), kind))
                })
                .collect(),
        )
    }

    fn kind(&self, name: &str) -> Option<ToolKind> {
        self.0
            .iter()
            .find(|(declared, _)| declared == name)
            .map(|(_, kind)| *kind)
    }

    fn names(&self) -> Vec<&str> {
        self.0.iter().map(|(name, _)| name.as_str()).collect()
    }
}

fn text_parts(content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => vec![json!({"type": "text", "text": text})],
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .map(|text| json!({"type": "text", "text": text}))
            .collect(),
        _ => Vec::new(),
    }
}

fn output_text(output: &Value) -> String {
    match output {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn custom_input(input: &str) -> Value {
    json!({CODE_FIELD: input})
}

fn call_input(item: &Value, tools: &Tools, name: &str) -> Result<Value, String> {
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("function_call {name} without arguments"))?;
    let parsed: Value = serde_json::from_str(arguments)
        .map_err(|error| format!("function_call {name} with unparsable arguments: {error}"))?;
    if tools.kind(name) == Some(ToolKind::CustomAsFunction) {
        let input = parsed
            .get(INPUT_FIELD)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("function_call {name} without its input string"))?;
        return Ok(custom_input(input));
    }
    Ok(parsed)
}

fn push(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut()
        && last.get("role").and_then(Value::as_str) == Some(role)
        && let Some(existing) = last.get_mut("content").and_then(Value::as_array_mut)
    {
        existing.extend(blocks);
        return;
    }
    messages.push(json!({"role": role, "content": blocks}));
}

pub fn messages(request: &Value, tools: &Tools) -> Result<Vec<Value>, String> {
    let input = request
        .get("input")
        .and_then(Value::as_array)
        .ok_or("a Responses request without an input list")?;
    let mut messages = Vec::new();
    for item in input {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("message");
        let text = |field: &str| item.get(field).and_then(Value::as_str);
        match kind {
            "message" => match text("role") {
                Some(ASSISTANT) => push(
                    &mut messages,
                    ASSISTANT,
                    text_parts(item.get("content").unwrap_or(&Value::Null)),
                ),
                Some(USER) => push(
                    &mut messages,
                    USER,
                    text_parts(item.get("content").unwrap_or(&Value::Null)),
                ),
                Some("developer" | "system") => {}
                role => return Err(format!("a Responses message with role {role:?}")),
            },
            "function_call" | "custom_tool_call" => {
                let name = text("name").ok_or_else(|| format!("a {kind} without a name"))?;
                let call_id = text("call_id").ok_or_else(|| format!("a {kind} without a call_id"))?;
                let input = if kind == "custom_tool_call" {
                    custom_input(text(INPUT_FIELD).ok_or_else(|| format!("{kind} {name} without input"))?)
                } else {
                    call_input(item, tools, name)?
                };
                push(
                    &mut messages,
                    ASSISTANT,
                    vec![json!({"type": "tool_use", "id": call_id, "name": name, "input": input})],
                );
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = text("call_id").ok_or_else(|| format!("a {kind} without a call_id"))?;
                let content = output_text(item.get("output").unwrap_or(&Value::Null));
                push(
                    &mut messages,
                    USER,
                    vec![json!({"type": "tool_result", "tool_use_id": call_id, "content": content})],
                );
            }
            "reasoning" => {}
            other => return Err(format!("a Responses input item of type {other}")),
        }
    }
    Ok(messages)
}

fn event(sequence: &mut u64, kind: &str, fields: Value) -> String {
    let mut event = Map::new();
    event.insert("type".to_owned(), json!(kind));
    event.insert("sequence_number".to_owned(), json!(*sequence));
    *sequence += 1;
    if let Value::Object(mut fields) = fields {
        event.append(&mut fields);
    }
    format!("event: {kind}\ndata: {}\n\n", Value::Object(event))
}

fn item_of(id: &str, block: &Value, tools: &Tools) -> Result<(Value, Value), String> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => {
            let text = block.get("text").and_then(Value::as_str).unwrap_or_default();
            let item = json!({"type": "message", "id": format!("msg_{id}"), "role": ASSISTANT, "status": "completed", "content": [{"type": "output_text", "text": text, "annotations": []}]});
            Ok((item, json!(text)))
        }
        Some("tool_use") => {
            let name = block.get("name").and_then(Value::as_str).unwrap_or_default();
            let call_id = block.get("id").and_then(Value::as_str).unwrap_or_default();
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            let code = || {
                input
                    .get(CODE_FIELD)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("the model's {name} call has no code"))
            };
            match tools.kind(name) {
                Some(ToolKind::Custom) => {
                    let code = code()?;
                    let item = json!({"type": "custom_tool_call", "id": format!("ctc_{id}"), "status": "completed", "call_id": call_id, "name": name, "input": code});
                    Ok((item, json!(code)))
                }
                Some(ToolKind::CustomAsFunction) => {
                    let arguments = json!({INPUT_FIELD: code()?}).to_string();
                    let item = json!({"type": "function_call", "id": format!("fc_{id}"), "status": "completed", "call_id": call_id, "name": name, "arguments": arguments});
                    Ok((item, json!(arguments)))
                }
                Some(ToolKind::Function) => {
                    let arguments = input.to_string();
                    let item = json!({"type": "function_call", "id": format!("fc_{id}"), "status": "completed", "call_id": call_id, "name": name, "arguments": arguments});
                    Ok((item, json!(arguments)))
                }
                None => Err(format!(
                    "the model calls {name}, which the request does not declare (it declares {:?})",
                    tools.names()
                )),
            }
        }
        other => Err(format!("the model answers with a {other:?} block")),
    }
}

pub fn events(model: &str, id: &str, message: &Value, tools: &Tools) -> Result<String, String> {
    let block = message
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| blocks.first())
        .ok_or("the model's message has no block")?;
    let (item, payload) = item_of(id, block, tools)?;
    let response_id = format!("resp_{id}");
    let mut sequence = 0;
    let mut body = event(
        &mut sequence,
        "response.created",
        json!({"response": {"id": response_id, "object": "response", "model": model, "status": "in_progress", "output": [], "usage": null}}),
    );
    let mut started = item.clone();
    let kind = item.get("type").and_then(Value::as_str).unwrap_or_default().to_owned();
    let item_id = item.get("id").cloned().unwrap_or(Value::Null);
    started["status"] = json!("in_progress");
    match kind.as_str() {
        "message" => started["content"] = json!([]),
        "function_call" => started["arguments"] = json!(""),
        _ => started[INPUT_FIELD] = json!(""),
    }
    body.push_str(&event(
        &mut sequence,
        "response.output_item.added",
        json!({"output_index": 0, "item": started}),
    ));
    let at = json!({"output_index": 0, "item_id": item_id});
    let with = |extra: Value| {
        let mut fields = at.clone();
        if let (Value::Object(fields), Value::Object(extra)) = (&mut fields, extra) {
            fields.extend(extra);
        }
        fields
    };
    match kind.as_str() {
        "message" => {
            body.push_str(&event(&mut sequence, "response.content_part.added", with(json!({"content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}}))));
            body.push_str(&event(&mut sequence, "response.output_text.delta", with(json!({"content_index": 0, "delta": payload}))));
            body.push_str(&event(&mut sequence, "response.output_text.done", with(json!({"content_index": 0, "text": payload}))));
            body.push_str(&event(&mut sequence, "response.content_part.done", with(json!({"content_index": 0, "part": {"type": "output_text", "text": payload, "annotations": []}}))));
        }
        "function_call" => {
            let call_id = item.get("call_id").cloned().unwrap_or(Value::Null);
            body.push_str(&event(&mut sequence, "response.function_call_arguments.delta", with(json!({"call_id": call_id, "delta": payload}))));
            body.push_str(&event(&mut sequence, "response.function_call_arguments.done", with(json!({"call_id": call_id, "arguments": payload}))));
        }
        _ => {
            let call_id = item.get("call_id").cloned().unwrap_or(Value::Null);
            body.push_str(&event(&mut sequence, "response.custom_tool_call_input.delta", with(json!({"call_id": call_id, "delta": payload}))));
            body.push_str(&event(&mut sequence, "response.custom_tool_call_input.done", with(json!({"call_id": call_id, "input": payload}))));
        }
    }
    body.push_str(&event(
        &mut sequence,
        "response.output_item.done",
        json!({"output_index": 0, "item": item}),
    ));
    body.push_str(&event(
        &mut sequence,
        "response.completed",
        json!({"response": {"id": response_id, "object": "response", "model": model, "status": "completed", "output": [item], "usage": {"input_tokens": 1, "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 1, "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 2}}}),
    ));
    Ok(body)
}

