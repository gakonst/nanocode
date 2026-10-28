use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use nanocode_sync::Atom;
use serde_json::{Map, Value, json};

use super::nested;
use crate::{
    ToolOutput, ToolOutputBody,
    contract::ToolOutputWire,
    embedded::{
        CodeModeExecution, CodeModeHostError, CodeModeNotification, CodeModeObserver,
        CodeModeUpdate, NestedToolCall,
    },
};

fn error(value: impl std::fmt::Display) -> CodeModeHostError {
    CodeModeHostError::new(value.to_string())
}

fn has_float(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.is_f64(),
        Value::Array(items) => items.iter().any(has_float),
        Value::Object(fields) => fields.values().any(has_float),
        _ => false,
    }
}

fn encodes(text: &str, value: &Value) -> bool {
    serde_json::to_string(value).is_ok_and(|encoded| encoded == text)
        && (!has_float(value)
            || serde_json::from_str::<Value>(text)
                .and_then(|parsed| serde_json::to_string(&parsed))
                .is_ok_and(|encoded| encoded == text))
}

fn prefix<'a>(text: &'a str, structured: &Value) -> Option<&'a str> {
    let output = structured
        .get("output")?
        .as_str()
        .filter(|output| !output.is_empty())?;
    text.strip_suffix(output)
}

fn joined(prefix: Value, structured: Option<&Value>) -> Result<String, CodeModeHostError> {
    match (
        prefix,
        structured
            .and_then(|structured| structured.get("output"))
            .and_then(Value::as_str),
    ) {
        (Value::String(mut text), Some(output)) => {
            text.push_str(output);
            Ok(text)
        }
        _ => Err(error(
            "output_prefix needs a string prefix and a structured result with a string output",
        )),
    }
}

pub fn output_to_wire(output: ToolOutput, compact: bool) -> Result<Value, CodeModeHostError> {
    encode_output(output, compact, false)
}

pub fn encode_output(
    output: ToolOutput,
    compact: bool,
    output_prefix: bool,
) -> Result<Value, CodeModeHostError> {
    let mut value = serde_json::to_value(output.into_wire().map_err(error)?).map_err(error)?;
    if let Some(fields) = value.as_object_mut() {
        if fields.get("structured_result") == fields.get("output") {
            fields.insert("structured_result".into(), Value::Null);
        } else if compact
            && let (Some(Value::String(text)), Some(structured)) =
                (fields.get("output"), fields.get("structured_result"))
            && !structured.is_null()
        {
            let head = if encodes(text, structured) {
                Some(None)
            } else if output_prefix {
                prefix(text, structured).map(|head| Some(head.to_owned()))
            } else {
                None
            };
            match head {
                Some(None) => {
                    fields.remove("output");
                }
                Some(Some(head)) => {
                    fields.remove("output");
                    fields.insert("output_prefix".into(), Value::String(head));
                }
                None => {}
            }
        }
    }
    Ok(value)
}

pub fn output_from_wire(mut value: Value) -> Result<ToolOutput, CodeModeHostError> {
    if let Some(fields) = value.as_object_mut()
        && !fields.contains_key("output")
    {
        if let Some(head) = fields.remove("output_prefix") {
            let text = joined(head, fields.get("structured_result"))?;
            fields.insert("output".into(), Value::String(text));
        } else if let Some(structured) = fields.get("structured_result") {
            let text = serde_json::to_string(structured).map_err(error)?;
            fields.insert("output".into(), Value::String(text));
        }
    }
    ToolOutput::from_wire(serde_json::from_value::<ToolOutputWire>(value).map_err(error)?)
        .map_err(error)
}

#[must_use]
pub fn answer_key(session_id: &str, call_id: &str) -> String {
    format!("{session_id}\u{0}{call_id}")
}

#[must_use]
pub fn asked_key(call_id: &str) -> String {
    format!("{call_id}\u{0}input")
}

#[derive(Default)]
pub struct Answers(Atom<HashMap<String, Arc<Value>>>);

impl Answers {
    pub fn record(&self, key: String, output: &ToolOutput) {
        let answer = Arc::new(json!({
            "output": output.output,
            "structured_result": output.structured_result(),
            "metadata": output.metadata,
        }));
        self.0.update(|answers| {
            answers.insert(key.clone(), Arc::clone(&answer));
        });
    }

    pub fn keep(&self, key: String, value: Value) {
        let value = Arc::new(value);
        self.0.update(|answers| {
            answers.insert(key.clone(), Arc::clone(&value));
        });
    }

    fn take(&self, key: &str) -> Option<Arc<Value>> {
        self.0.try_update(|answers| answers.remove(key))
    }

    pub fn forget(&self, prefix: &str) {
        self.0.try_update(|answers| {
            let before = answers.len();
            answers.retain(|key, _| !key.starts_with(prefix));
            (answers.len() != before).then_some(())
        });
    }
}

#[derive(Default)]
pub struct SentCalls {
    compact: bool,
    output_prefix: bool,
    answers: Option<Arc<Answers>>,
    asks: Option<Arc<Answers>>,
    started: HashSet<String>,
    completed: HashSet<String>,
}

impl SentCalls {
    #[must_use]
    pub fn new(compact: bool) -> Self {
        Self {
            compact,
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn output_prefix(mut self, output_prefix: bool) -> Self {
        self.output_prefix = output_prefix;
        self
    }

    #[must_use]
    pub fn answers(mut self, answers: Option<Arc<Answers>>) -> Self {
        self.answers = answers;
        self
    }

    #[must_use]
    pub fn asks(mut self, asks: Option<Arc<Answers>>) -> Self {
        self.asks = asks;
        self
    }

    #[must_use]
    pub fn update(&self, update: &CodeModeUpdate<'_>) -> Value {
        match update {
            CodeModeUpdate::NestedCallStarted {
                call_id,
                name,
                input,
            } => {
                if self
                    .asks
                    .as_ref()
                    .and_then(|asks| asks.take(&asked_key(call_id)))
                    .is_some_and(|asked| *asked == **input)
                {
                    json!({"call_id":call_id,"name":name,"asked":true})
                } else {
                    json!({"call_id":call_id,"name":name,"input":input})
                }
            }
            CodeModeUpdate::NestedCallCompleted(call) => json!({"completed":self.call(call)}),
            CodeModeUpdate::Notification(notification) => {
                json!({"notification":{"call_id":notification.call_id,"text":notification.text}})
            }
        }
    }

    pub fn sent(&mut self, update: &CodeModeUpdate<'_>) {
        if !self.compact {
            return;
        }
        match update {
            CodeModeUpdate::NestedCallStarted { call_id, .. } => {
                self.started.insert((*call_id).to_owned());
            }
            CodeModeUpdate::NestedCallCompleted(call) => {
                self.completed.insert(call.call_id.clone());
            }
            CodeModeUpdate::Notification(_) => {}
        }
    }

    #[must_use]
    pub fn execution(&self, execution: CodeModeExecution) -> Value {
        self.encode(&execution)
    }

    #[must_use]
    pub fn encode(&self, execution: &CodeModeExecution) -> Value {
        let calls = execution
            .nested_calls
            .iter()
            .map(|call| {
                if self.completed.contains(&call.call_id) {
                    json!({"call_id":call.call_id,"delivered":true})
                } else {
                    self.call(call)
                }
            })
            .collect::<Vec<_>>();
        json!({"cell":execution.cell,"output":execution.output,"success":execution.success,"nested_calls":calls,"notifications":execution.notifications.iter().map(|n| json!({"call_id":n.call_id,"text":n.text})).collect::<Vec<_>>()})
    }

    fn call(&self, call: &NestedToolCall) -> Value {
        if !self.compact {
            return nested(call);
        }
        if let Some(asks) = &self.asks {
            asks.take(&asked_key(&call.call_id));
        }
        let mut fields = Map::new();
        fields.insert("call_id".into(), Value::String(call.call_id.clone()));
        fields.insert("name".into(), Value::String(call.name.clone()));
        if !self.started.contains(&call.call_id) {
            fields.insert("input".into(), call.input.clone());
        }
        let output = serde_json::to_value(&call.output).unwrap_or(Value::Null);
        let metadata = serde_json::to_value(&call.metadata).unwrap_or(Value::Null);
        if let Some(answer) = self
            .answers
            .as_ref()
            .and_then(|answers| answers.take(&call.call_id))
            && answer.get("output") == Some(&output)
            && answer.get("structured_result") == Some(&call.structured_result)
            && answer.get("metadata") == Some(&metadata)
        {
            fields.insert("answered".into(), Value::Bool(true));
            fields.insert("success".into(), Value::Bool(call.success));
            fields.insert("started_after_ns".into(), json!(call.started_after_ns));
            fields.insert("duration_ns".into(), json!(call.duration_ns));
            return Value::Object(fields);
        }
        if output != call.structured_result {
            let head = match &call.output {
                ToolOutputBody::Text(text) if self.output_prefix => {
                    prefix(text, &call.structured_result)
                }
                _ => None,
            };
            if matches!(&call.output, ToolOutputBody::Text(text) if encodes(text, &call.structured_result))
            {
                fields.insert("structured_result".into(), call.structured_result.clone());
            } else if let Some(head) = head {
                fields.insert("output_prefix".into(), Value::String(head.to_owned()));
                fields.insert("structured_result".into(), call.structured_result.clone());
            } else {
                fields.insert("output".into(), output);
                fields.insert("structured_result".into(), call.structured_result.clone());
            }
        } else {
            fields.insert("output".into(), output);
        }
        fields.insert("success".into(), Value::Bool(call.success));
        fields.insert("started_after_ns".into(), json!(call.started_after_ns));
        fields.insert("duration_ns".into(), json!(call.duration_ns));
        fields.insert("metadata".into(), metadata);
        Value::Object(fields)
    }
}

#[derive(Default)]
pub struct ReceivedCalls {
    started: HashMap<String, Value>,
    completed: HashMap<String, Value>,
    answers: Option<(Arc<Answers>, String)>,
}

fn call_id(fields: &Map<String, Value>) -> String {
    fields
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn expand_result(fields: &mut Map<String, Value>) -> Result<(), CodeModeHostError> {
    if let Some(head) = fields.remove("output_prefix") {
        let text = joined(head, fields.get("structured_result"))?;
        fields.insert("output".into(), Value::String(text));
        return Ok(());
    }
    match (fields.get("output"), fields.get("structured_result")) {
        (Some(_), Some(_)) => {}
        (Some(output), None) => {
            let output = output.clone();
            fields.insert("structured_result".into(), output);
        }
        (None, Some(structured)) => {
            let text = serde_json::to_string(structured).map_err(error)?;
            fields.insert("output".into(), Value::String(text));
        }
        (None, None) => {
            return Err(error(
                "nested call omits both its output and its structured result",
            ));
        }
    }
    Ok(())
}

impl ReceivedCalls {
    #[must_use]
    pub fn answered(mut self, answers: Arc<Answers>, session_id: impl Into<String>) -> Self {
        self.answers = Some((answers, session_id.into()));
        self
    }

    fn asked(&self, call_id: &str) -> Option<Arc<Value>> {
        self.answers
            .as_ref()
            .and_then(|(answers, session_id)| answers.take(&answer_key(session_id, &asked_key(call_id))))
    }

    fn expand(&mut self, call: &mut Value) -> Result<(), CodeModeHostError> {
        let Some(fields) = call.as_object_mut() else {
            return Ok(());
        };
        self.asked(&call_id(fields));
        if fields.remove("answered").is_some() {
            let id = call_id(fields);
            let answer = self
                .answers
                .as_ref()
                .and_then(|(answers, session_id)| answers.take(&answer_key(session_id, &id)))
                .ok_or_else(|| {
                    error(format!(
                        "nested call {id} omits a callback result that this brain did not record"
                    ))
                })?;
            if let Value::Object(answer) = Arc::unwrap_or_clone(answer) {
                fields.extend(answer);
            }
        }
        if !fields.contains_key("input") {
            let id = call_id(fields);
            let input = self.started.remove(&id).ok_or_else(|| {
                error(format!(
                    "nested call {id} omits an input that no start update carried"
                ))
            })?;
            fields.insert("input".into(), input);
        }
        expand_result(fields)
    }

    pub fn deliver<'o>(
        &mut self,
        observer: Option<&mut (dyn CodeModeObserver + 'o)>,
        mut update: Value,
    ) -> Result<(), CodeModeHostError> {
        if let Some(call) = update.get_mut("completed") {
            self.expand(call)?;
            if let Some(observer) = observer {
                let call = serde_json::from_value::<NestedToolCall>(update["completed"].clone())
                    .map_err(error)?;
                observer.update(CodeModeUpdate::NestedCallCompleted(&call));
            }
            if let Some(Value::Object(fields)) = update
                .as_object_mut()
                .and_then(|update| update.remove("completed"))
            {
                self.completed
                    .insert(call_id(&fields), Value::Object(fields));
            }
            return Ok(());
        }
        if let Some(notification) = update.get("notification") {
            if let Some(observer) = observer {
                let notification =
                    serde_json::from_value::<CodeModeNotification>(notification.clone())
                        .map_err(error)?;
                observer.update(CodeModeUpdate::Notification(&notification));
            }
            return Ok(());
        }
        let call_id = update["call_id"]
            .as_str()
            .ok_or_else(|| error("missing update call_id"))?
            .to_owned();
        let asked = self.asked(&call_id);
        if let Some(fields) = update.as_object_mut()
            && fields.remove("asked").is_some()
        {
            let input = asked.ok_or_else(|| {
                error(format!(
                    "nested call {call_id} omits an input that no callback carried"
                ))
            })?;
            fields.insert("input".into(), Arc::unwrap_or_clone(input));
        }
        let name = update["name"]
            .as_str()
            .ok_or_else(|| error("missing update name"))?;
        if let Some(observer) = observer {
            observer.update(CodeModeUpdate::NestedCallStarted {
                call_id: &call_id,
                name,
                input: &update["input"],
            });
        }
        if let Some(input) = update.get_mut("input") {
            self.started.insert(call_id, input.take());
        }
        Ok(())
    }

    pub fn execution(&mut self, execution: &mut Value) -> Result<(), CodeModeHostError> {
        let Some(Value::Array(calls)) = execution.get_mut("nested_calls") else {
            return Ok(());
        };
        for call in calls {
            if call.get("delivered").and_then(Value::as_bool) == Some(true) {
                let id = call.as_object().map(call_id).unwrap_or_default();
                *call = self.completed.remove(&id).ok_or_else(|| {
                    error(format!(
                        "remote reply references nested call {id}, which no update carried"
                    ))
                })?;
            } else {
                self.expand(call)?;
            }
        }
        Ok(())
    }
}
