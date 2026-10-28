use super::*;
use crate::client::responses::FunctionOutputContent;
use arc_swap::{ArcSwap, ArcSwapOption};
use nanocode_code_mode::code_mode::NestedToolCall;
use nanocode_code_mode::contract::ToolOutputContent;
use nanocode_sync::Slot;

#[derive(Clone)]
pub(super) struct ActiveToolCall {
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) kind: CodeCallKind,
    pub(super) started_at: Instant,
    pub(super) shell_abort_format: bool,
    pub(super) completion: Arc<Slot<CompletedToolCall>>,
    pub(super) progress: Arc<ArcSwap<ActiveToolProgress>>,
    pub(super) execution_started_at: Arc<ArcSwapOption<Instant>>,
    pub(super) span: tracing::Span,
}

#[derive(Clone, Default)]
pub(super) struct ActiveToolProgress {
    pub(super) nested_tool_calls: u32,
    pub(super) active_nested_tool_calls: Vec<ActiveNestedToolCall>,
}

#[derive(Clone)]
pub(super) struct ActiveNestedToolCall {
    pub(super) call_id: String,
    pub(super) tool: String,
    pub(super) started_at: Instant,
}

impl ActiveNestedToolCall {
    pub(super) fn started_after_ns(&self, parent_started_at: Instant) -> u64 {
        u64::try_from(
            self.started_at
                .saturating_duration_since(parent_started_at)
                .as_nanos(),
        )
        .unwrap_or(u64::MAX)
    }
}

#[derive(Deserialize)]
#[serde(try_from = "CompletedToolCallRecord")]
pub(super) struct CompletedToolCall {
    pub(super) cell: Option<nanocode_code_mode::code_mode::CodeModeCell>,
    pub(super) call_id: String,
    pub(super) tool: String,
    pub(super) success: bool,
    pub(super) duration_ns: u64,
    pub(super) work_duration_ns: u64,
    pub(super) output: ToolOutputBody,
    pub(super) structured_result: Value,
    pub(super) metadata: Option<Box<RawValue>>,
    pub(super) response_items: Vec<ResponseItem>,
}

#[derive(Deserialize)]
struct CompletedToolCallRecord {
    cell: Option<nanocode_code_mode::code_mode::CodeModeCell>,
    call_id: String,
    tool: String,
    success: bool,
    duration_ns: u64,
    work_duration_ns: u64,
    #[serde(default)]
    output: Option<ToolOutputBody>,
    #[serde(default, deserialize_with = "present_value")]
    structured_result: Option<Value>,
    metadata: Option<Box<RawValue>>,
    response_items: Vec<ResponseItem>,
}

fn present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl TryFrom<CompletedToolCallRecord> for CompletedToolCall {
    type Error = &'static str;

    fn try_from(record: CompletedToolCallRecord) -> std::result::Result<Self, Self::Error> {
        let output = match record.output {
            Some(output) => output,
            None => primary_output(&record.response_items)
                .and_then(restored_output)
                .ok_or("tool step record has no output")?,
        };
        let structured_result = record
            .structured_result
            .unwrap_or_else(|| output.structured_result());
        Ok(Self {
            cell: record.cell,
            call_id: record.call_id,
            tool: record.tool,
            success: record.success,
            duration_ns: record.duration_ns,
            work_duration_ns: record.work_duration_ns,
            output,
            structured_result,
            metadata: record.metadata,
            response_items: record.response_items,
        })
    }
}

impl Serialize for CompletedToolCall {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Record<'a> {
            cell: &'a Option<nanocode_code_mode::code_mode::CodeModeCell>,
            call_id: &'a str,
            tool: &'a str,
            success: bool,
            duration_ns: u64,
            work_duration_ns: u64,
            #[serde(skip_serializing_if = "Option::is_none")]
            output: Option<&'a ToolOutputBody>,
            #[serde(skip_serializing_if = "Option::is_none")]
            structured_result: Option<&'a Value>,
            metadata: &'a Option<Box<RawValue>>,
            response_items: &'a [ResponseItem],
        }

        let output_in_items = primary_output(&self.response_items)
            .is_some_and(|primary| restorable_output(&self.output, primary));
        let derived_structured_result = match (&self.output, &self.structured_result) {
            (ToolOutputBody::Text(text), Value::String(value)) => text == value,
            (ToolOutputBody::Text(_), _) => false,
            (ToolOutputBody::Content(_), value) => self.output.structured_result() == *value,
        };
        Record {
            cell: &self.cell,
            call_id: &self.call_id,
            tool: &self.tool,
            success: self.success,
            duration_ns: self.duration_ns,
            work_duration_ns: self.work_duration_ns,
            output: (!output_in_items).then_some(&self.output),
            structured_result: (!derived_structured_result).then_some(&self.structured_result),
            metadata: &self.metadata,
            response_items: &self.response_items,
        }
        .serialize(serializer)
    }
}

fn primary_output(items: &[ResponseItem]) -> Option<&FunctionOutputBody> {
    match items.first()? {
        ResponseItem::FunctionCallOutput { output, .. }
        | ResponseItem::CustomToolCallOutput { output, .. } => Some(output),
        _ => None,
    }
}

fn restorable_output(output: &ToolOutputBody, primary: &FunctionOutputBody) -> bool {
    match (output, primary) {
        (ToolOutputBody::Text(text), FunctionOutputBody::Text(primary)) => text == &**primary,
        (ToolOutputBody::Content(content), FunctionOutputBody::Content(primary)) => {
            content.len() == primary.len()
                && content
                    .iter()
                    .zip(primary)
                    .all(|(part, primary)| match (part, primary) {
                        (
                            ToolOutputContent::InputText { text },
                            FunctionOutputContent::InputText { text: primary },
                        ) => text == &**primary,
                        (
                            ToolOutputContent::InputAudio { audio_url },
                            FunctionOutputContent::InputAudio { audio_url: primary },
                        ) => audio_url == &**primary,
                        (
                            ToolOutputContent::EncryptedContent { encrypted_content },
                            FunctionOutputContent::EncryptedContent {
                                encrypted_content: primary,
                            },
                        ) => encrypted_content == &**primary,
                        _ => false,
                    })
        }
        _ => false,
    }
}

fn restored_output(primary: &FunctionOutputBody) -> Option<ToolOutputBody> {
    match primary {
        FunctionOutputBody::Text(text) => Some(ToolOutputBody::Text(text.to_string())),
        FunctionOutputBody::Content(content) => content
            .iter()
            .map(|part| match part {
                FunctionOutputContent::InputText { text } => Some(ToolOutputContent::InputText {
                    text: text.to_string(),
                }),
                FunctionOutputContent::InputAudio { audio_url } => {
                    Some(ToolOutputContent::InputAudio {
                        audio_url: audio_url.to_string(),
                    })
                }
                FunctionOutputContent::EncryptedContent { encrypted_content } => {
                    Some(ToolOutputContent::EncryptedContent {
                        encrypted_content: encrypted_content.to_string(),
                    })
                }
                FunctionOutputContent::InputImage { .. }
                | FunctionOutputContent::InputImageFile { .. } => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(ToolOutputBody::Content),
    }
}

pub(super) struct NestedToolEventObserver<'a> {
    events: &'a EventSink,
    tool_call_indices: &'a HashMap<Box<str>, u32>,
    pub(super) progress: &'a ArcSwap<ActiveToolProgress>,
    fallback_call_index: u32,
    parent_call_id: &'a str,
    error: Option<NanocodeError>,
}

impl CodeModeObserver for NestedToolEventObserver<'_> {
    fn update(&mut self, update: CodeModeUpdate<'_>) {
        if self.error.is_some() {
            return;
        }
        let result = match update {
            CodeModeUpdate::NestedCallStarted {
                call_id,
                name,
                input,
            } => {
                let started_at = Instant::now();
                let (call_id, call_index) = self.event_context(call_id);
                let logged = logged_arguments(input);
                let result = self.events.emit(
                    AgentEventKind::ToolCall,
                    ToolCallEvent {
                        call_id: &call_id,
                        tool: name,
                        arguments: logged.as_ref().map_or(input, |(arguments, _)| arguments),
                        model_call_index: call_index,
                        truncated: logged.is_some(),
                        original_bytes: logged.as_ref().map(|(_, bytes)| *bytes),
                    },
                );
                if result.is_ok() {
                    let nested = ActiveNestedToolCall {
                        call_id,
                        tool: name.to_owned(),
                        started_at,
                    };
                    self.progress.rcu(|progress| {
                        let mut progress = ActiveToolProgress::clone(progress);
                        progress.nested_tool_calls += 1;
                        progress.active_nested_tool_calls.push(nested.clone());
                        progress
                    });
                }
                result
            }
            CodeModeUpdate::NestedCallCompleted(call) => {
                let (call_id, _) = self.event_context(&call.call_id);
                self.progress.rcu(|progress| {
                    let mut progress = ActiveToolProgress::clone(progress);
                    if let Some(index) = progress
                        .active_nested_tool_calls
                        .iter()
                        .position(|active| active.call_id == call_id)
                    {
                        progress.active_nested_tool_calls.remove(index);
                    }
                    progress
                });

                let logged = logged_nested_result(call);
                self.events.emit(
                    AgentEventKind::ToolResult,
                    ToolResultEvent {
                        call_id: &call_id,
                        tool: &call.name,
                        status: status(call.success),
                        duration_ns: call.duration_ns,
                        started_after_ns: Some(call.started_after_ns),
                        result: logged
                            .as_ref()
                            .map_or(&call.output, |logged| &logged.result),
                        structured_result: logged
                            .as_ref()
                            .map_or(&call.structured_result, |logged| &logged.structured_result),
                        metadata: match &logged {
                            Some(logged) if !logged.metadata => None,
                            _ => call.metadata.as_deref(),
                        },
                        cell: None,
                        truncated: logged.is_some(),
                        original_bytes: logged.as_ref().map(|logged| logged.original_bytes),
                    },
                )
            }
            CodeModeUpdate::Notification(notification) => self.events.emit(
                AgentEventKind::ToolNotification,
                ToolNotificationEvent {
                    call_id: &notification.call_id,
                    text: &notification.text,
                },
            ),
        };
        if let Err(error) = result {
            self.error = Some(error.into());
        }
    }
}

const LOGGED_RESULT_BYTES: usize = 32 * 1024;
const LOGGED_PREVIEW_BYTES: usize = 16 * 1024;
const LOGGED_METADATA_BYTES: usize = 4096;

struct LoggedResult {
    result: ToolOutputBody,
    structured_result: Value,
    metadata: bool,
    original_bytes: usize,
}

fn encoded_len(value: &Value) -> usize {
    match value {
        Value::Null => 0,
        Value::String(text) => text.len(),
        value => serde_json::to_string(value).map_or(0, |text| text.len()),
    }
}

fn utf8_prefix(text: &str, bytes: usize) -> &str {
    &text[..text.floor_char_boundary(bytes)]
}

fn output_prefix(output: &ToolOutputBody, bytes: usize) -> ToolOutputBody {
    match output {
        ToolOutputBody::Text(text) => ToolOutputBody::Text(utf8_prefix(text, bytes).to_owned()),
        ToolOutputBody::Content(content) => {
            let mut budget = bytes;
            ToolOutputBody::Content(
                content
                    .iter()
                    .map(|item| match item {
                        ToolOutputContent::InputText { text } => {
                            let kept = utf8_prefix(text, budget);
                            budget -= kept.len();
                            ToolOutputContent::InputText {
                                text: kept.to_owned(),
                            }
                        }
                        item => item.clone(),
                    })
                    .collect(),
            )
        }
    }
}

fn structured_reference(structured: &Value) -> Value {
    match structured {
        Value::Object(object) => {
            let reference = ["image_url", "file_id"]
                .into_iter()
                .filter_map(|key| {
                    object
                        .get(key)
                        .filter(|field| field.is_string())
                        .map(|field| (key.to_owned(), field.clone()))
                })
                .collect::<serde_json::Map<_, _>>();
            if reference.is_empty() {
                Value::Null
            } else {
                Value::Object(reference)
            }
        }
        _ => Value::Null,
    }
}

fn logged_nested_result(call: &NestedToolCall) -> Option<LoggedResult> {
    let structured = &call.structured_result;
    let (output_bytes, duplicate) = match &call.output {
        ToolOutputBody::Text(text) => (
            text.len(),
            structured.as_str() == Some(text.as_str())
                || (text.len() == encoded_len(structured)
                    && serde_json::from_str::<Value>(text).ok().as_ref() == Some(structured)),
        ),
        ToolOutputBody::Content(content) => {
            let bytes = serde_json::to_string(content).map_or(0, |text| text.len());
            (bytes, bytes == encoded_len(structured))
        }
    };
    let metadata_bytes = call.metadata.as_deref().map_or(0, |raw| raw.get().len());
    let original_bytes = output_bytes
        + if duplicate {
            0
        } else {
            encoded_len(structured)
        }
        + metadata_bytes;
    if original_bytes <= LOGGED_RESULT_BYTES {
        return None;
    }
    Some(LoggedResult {
        result: output_prefix(&call.output, LOGGED_PREVIEW_BYTES),
        structured_result: structured_reference(structured),
        metadata: metadata_bytes <= LOGGED_METADATA_BYTES,
        original_bytes,
    })
}

fn visible_output(output: &ToolOutputBody) -> Option<ToolOutputBody> {
    use crate::client::session::{TOOL_OUTPUT_TOKEN_LIMIT, truncate_middle_with_token_budget};
    match output {
        ToolOutputBody::Text(text) => (text.len().div_ceil(4) > TOOL_OUTPUT_TOKEN_LIMIT).then(|| {
            ToolOutputBody::Text(truncate_middle_with_token_budget(
                text,
                TOOL_OUTPUT_TOKEN_LIMIT,
            ))
        }),
        ToolOutputBody::Content(content) => {
            let mut remaining = TOOL_OUTPUT_TOKEN_LIMIT;
            let mut omitted = 0_usize;
            let mut changed = false;
            let mut visible = Vec::with_capacity(content.len());
            for item in content {
                match item {
                    ToolOutputContent::InputText { text } => {
                        if remaining == 0 {
                            omitted += 1;
                            continue;
                        }
                        let tokens = text.len().div_ceil(4);
                        if tokens <= remaining {
                            remaining -= tokens;
                            visible.push(item.clone());
                            continue;
                        }
                        changed = true;
                        let kept = truncate_middle_with_token_budget(text, remaining);
                        if kept.is_empty() {
                            omitted += 1;
                        } else {
                            visible.push(ToolOutputContent::InputText { text: kept });
                        }
                        remaining = 0;
                    }
                    ToolOutputContent::InputAudio { .. } => changed = true,
                    item => visible.push(item.clone()),
                }
            }
            if omitted > 0 {
                visible.push(ToolOutputContent::InputText {
                    text: format!("[omitted {omitted} text items ...]"),
                });
            }
            (changed || omitted > 0).then_some(ToolOutputBody::Content(visible))
        }
    }
}

fn logged_top_level_result(completed: &CompletedToolCall) -> Option<LoggedResult> {
    let visible = visible_output(&completed.output);
    let text_bytes = match &completed.output {
        ToolOutputBody::Text(text) => text.len(),
        ToolOutputBody::Content(content) => content
            .iter()
            .map(|item| match item {
                ToolOutputContent::InputText { text } => text.len(),
                _ => 0,
            })
            .sum(),
    };
    let structured_bytes = encoded_len(&completed.structured_result);
    let metadata_bytes = completed
        .metadata
        .as_deref()
        .map_or(0, |raw| raw.get().len());
    if visible.is_none()
        && structured_bytes <= LOGGED_RESULT_BYTES
        && metadata_bytes <= LOGGED_RESULT_BYTES
    {
        return None;
    }
    Some(LoggedResult {
        result: visible.unwrap_or_else(|| completed.output.clone()),
        structured_result: if structured_bytes > LOGGED_RESULT_BYTES {
            structured_reference(&completed.structured_result)
        } else {
            completed.structured_result.clone()
        },
        metadata: metadata_bytes <= LOGGED_RESULT_BYTES,
        original_bytes: text_bytes + structured_bytes + metadata_bytes,
    })
}

fn logged_arguments(input: &Value) -> Option<(Value, usize)> {
    fn lengths(value: &Value, found: &mut Vec<usize>) {
        match value {
            Value::String(text) => found.push(text.len()),
            Value::Array(items) => items.iter().for_each(|item| lengths(item, found)),
            Value::Object(fields) => fields.values().for_each(|item| lengths(item, found)),
            _ => {}
        }
    }
    fn cut(value: &Value, cap: usize) -> Value {
        match value {
            Value::String(text) => Value::String(utf8_prefix(text, cap).to_owned()),
            Value::Array(items) => Value::Array(items.iter().map(|item| cut(item, cap)).collect()),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, item)| (key.clone(), cut(item, cap)))
                    .collect(),
            ),
            value => value.clone(),
        }
    }
    let size = encoded_len(input);
    if size <= LOGGED_RESULT_BYTES {
        return None;
    }
    let mut found = Vec::new();
    lengths(input, &mut found);
    let Some(mut left) = LOGGED_PREVIEW_BYTES.checked_sub(size - found.iter().sum::<usize>())
    else {
        return Some((Value::Null, size));
    };
    found.sort_unstable();
    let mut cap = usize::MAX;
    for (index, length) in found.iter().enumerate() {
        let remaining = found.len() - index;
        if length * remaining > left {
            cap = left / remaining;
            break;
        }
        left -= length;
    }
    Some((cut(input, cap), size))
}

pub(super) struct DetachedNestedToolEvents {
    events: EventSink,
    tool_call_indices: HashMap<Box<str>, u32>,
    progress: ArcSwap<ActiveToolProgress>,
    parent_call_id: String,
}

impl DetachedNestedToolEvents {
    pub(super) fn new(
        events: EventSink,
        tool_call_indices: HashMap<Box<str>, u32>,
        parent_call_id: &str,
    ) -> Self {
        Self {
            events,
            tool_call_indices,
            progress: ArcSwap::default(),
            parent_call_id: parent_call_id.to_owned(),
        }
    }
}

impl CodeModeObserver for DetachedNestedToolEvents {
    fn update(&mut self, update: CodeModeUpdate<'_>) {
        let fallback_call_index = self
            .tool_call_indices
            .get(self.parent_call_id.as_str())
            .copied()
            .unwrap_or_default();
        let mut observer = NestedToolEventObserver {
            events: &self.events,
            tool_call_indices: &self.tool_call_indices,
            progress: &self.progress,
            fallback_call_index,
            parent_call_id: &self.parent_call_id,
            error: None,
        };
        observer.update(update);
        if let Some(error) = observer.error {
            tracing::debug!(%error, "detached Code Mode update was not emitted");
        }
    }
}

pub(super) struct TurnTeardownEvents<'a> {
    events: &'a EventSink,
    tool_call_indices: &'a HashMap<Box<str>, u32>,
    active: Vec<Arc<ArcSwap<ActiveToolProgress>>>,
    progress: ArcSwap<ActiveToolProgress>,
    pub(super) terminated: Vec<(String, String)>,
}

impl<'a> TurnTeardownEvents<'a> {
    pub(super) fn new(
        events: &'a EventSink,
        tool_call_indices: &'a HashMap<Box<str>, u32>,
        active: &[ActiveToolCall],
    ) -> Self {
        Self {
            events,
            tool_call_indices,
            active: active.iter().map(|call| call.progress.clone()).collect(),
            progress: ArcSwap::default(),
            terminated: Vec::new(),
        }
    }
}

impl CodeModeObserver for TurnTeardownEvents<'_> {
    fn cell_terminated(&mut self, origin_call_id: &str, output: &str) {
        self.terminated
            .push((origin_call_id.to_owned(), output.to_owned()));
    }

    fn update(&mut self, update: CodeModeUpdate<'_>) {
        let mut observer = NestedToolEventObserver {
            events: self.events,
            tool_call_indices: self.tool_call_indices,
            progress: &self.progress,
            fallback_call_index: 0,
            parent_call_id: "",
            error: None,
        };
        if let CodeModeUpdate::NestedCallCompleted(call) = &update {
            let (call_id, _) = observer.event_context(&call.call_id);
            let owned_by_active_call = self.active.iter().any(|progress| {
                progress
                    .load()
                    .active_nested_tool_calls
                    .iter()
                    .any(|active| active.call_id == call_id)
            });
            if owned_by_active_call {
                return;
            }
        }
        observer.update(update);
        if let Some(error) = observer.error {
            tracing::debug!(%error, "terminated Code Mode update was not emitted");
        }
    }
}

impl NestedToolEventObserver<'_> {
    fn event_context(&self, nested_call_id: &str) -> (String, u32) {
        let embedded_parent = nested_call_id
            .rsplit_once("/code-")
            .map(|(parent, _)| parent);
        let original_parent = embedded_parent.unwrap_or(self.parent_call_id);
        let call_id = embedded_parent.map_or_else(
            || format!("{}/{nested_call_id}", self.parent_call_id),
            |_| nested_call_id.to_owned(),
        );
        let call_index = self
            .tool_call_indices
            .get(original_parent)
            .copied()
            .unwrap_or(self.fallback_call_index);
        (call_id, call_index)
    }
}

async fn execute_code_call(
    tools: &ToolRuntime,
    call: &CodeCall,
    owned_context: Option<OwnedToolContext>,
    context: ToolContext<'_>,
    observer: &mut dyn CodeModeObserver,
    tool_span: &tracing::Span,
    recover: bool,
) -> Result<Option<CodeModeExecution>> {
    if recover {
        let recovered = tools
            .recover_code(context)
            .instrument(tool_span.clone())
            .await
            .unwrap_or(None);
        for nested in recovered
            .iter()
            .flat_map(|execution| &execution.nested_calls)
        {
            observer.update(CodeModeUpdate::NestedCallStarted {
                call_id: &nested.call_id,
                name: &nested.name,
                input: &nested.input,
            });
            observer.update(CodeModeUpdate::NestedCallCompleted(nested));
        }
        return Ok(recovered);
    }
    let execution = if let Some(context) = owned_context {
        tools
            .execute_code_owned_with_updates(&call.input, context, observer)
            .instrument(tool_span.clone())
            .await
            .map_err(interrupted_tool_host)?
    } else {
        tools
            .wait_for_code_with_updates(&call.input, context, observer)
            .instrument(tool_span.clone())
            .await
            .map_err(interrupted_tool_host)?
    };
    Ok(Some(execution))
}

fn interrupted_tool_host(error: nanocode_code_mode::embedded::CodeModeHostError) -> NanocodeError {
    NanocodeError::execution_policy_with_disposition(
        "tool host interrupted",
        crate::ExecutionPolicyDisposition::Reopen,
        error,
    )
}

impl ModelRun {
    pub(super) async fn execute_model_tools(
        &mut self,
        tools: &ToolRuntime,
        conversation: &mut ConversationState,
        call_index: u32,
        calls: Vec<CodeCall>,
        history: Option<Arc<SharedHistory>>,
        turn_id: String,
    ) -> Result<()> {
        self.active_tool_batch_started_at = Some(Instant::now());
        let mut prepared = Vec::with_capacity(calls.len());
        for call in calls {
            let active = self.prepare_model_tool_call(call_index, &call)?;
            prepared.push((call, active));
        }

        let events = self.events.clone();
        let tool_call_indices = self.tool_call_indices.clone();
        let session_id = events.request_id().to_owned();
        let model = self.model;
        let host_context = self.host_context.clone();

        let instruction_revision = self.instruction_revision;
        let execution_steps = self.execution_steps.clone();
        let images = self.images.clone();
        let solo = prepared.len() == 1;
        let mut executions = prepared
            .into_iter()
            .map(|(call, active)| {
                let history = history.clone();
                let events = events.clone();
                let tool_call_indices = tool_call_indices.clone();
                let session_id = session_id.clone();
                let turn_id = turn_id.clone();
                let host_context = host_context.clone();
                let execution_steps = execution_steps.clone();
                let images = images.clone();
                async move {
                    let started_at = active.started_at;
                    let step_id = format!("tool-{call_index}-{}", call.call_id);
                    let mut interrupted = false;
                    let mut recover = false;
                    let recovered = if let Some(steps) = &execution_steps {
                        match steps
                            .begin::<_, CompletedToolCall>(&step_id, "tool_call", &call)
                            .await?
                        {
                            crate::agent::ExecutionStep::Execute => None,
                            crate::agent::ExecutionStep::Interrupted => {
                                interrupted = true;
                                if !matches!(call.kind, CodeCallKind::ToolSearch) {
                                    recover = true;
                                    None
                                } else {
                                    Some(Self::interrupted_tool_call(
                                        active.kind,
                                        &active.call_id,
                                        &active.name,
                                    ))
                                }
                            }
                            crate::agent::ExecutionStep::Replay(output) => Some(output),
                        }
                    } else {
                        None
                    };
                    let (result, persist_result) = if let Some(mut completed) = recovered {
                        prepare_output_images(&mut completed.output).await;
                        nanocode_code_mode::image::prepare_history_images(
                            &mut completed.response_items,
                        );
                        (Ok(completed), interrupted)
                    } else {
                        let dispatch = async {
                            active
                                .execution_started_at
                                .store(Some(Arc::new(Instant::now())));
                            Self::execute_model_tool_call(
                                tools,
                                &images,
                                &events,
                                &tool_call_indices,
                                call_index,
                                call,
                                history,
                                &session_id,
                                &turn_id,
                                model,
                                host_context.as_deref(),
                                instruction_revision,
                                started_at,
                                &active.progress,
                                &active.span,
                                recover,
                            )
                            .await
                        };
                        let result = match AssertUnwindSafe(dispatch).catch_unwind().await {
                            Ok(result) => result,
                            Err(payload) => Ok(Self::panicked_tool_call(&active, payload)),
                        };
                        (result, true)
                    };
                    match result {
                        Ok(mut completed) => {
                            if persist_result {
                                completed.work_duration_ns =
                                    Self::completed_tool_work_duration(&active);
                                match &execution_steps {
                                    Some(steps) if solo => {
                                        steps.complete_deferred(&step_id, &completed).await?;
                                    }
                                    Some(steps) => steps.complete(&step_id, &completed).await?,
                                    None => {}
                                }
                            }
                            Self::emit_completed_tool_result(&events, &completed)?;
                            drop(active.completion.put(completed));
                            Ok(active)
                        }
                        Err(error) => Err(error),
                    }
                }
            })
            .collect::<FuturesOrdered<_>>();
        while let Some(active) = executions.next().await {
            let active = active?;
            let Some(completed) = active.completion.take() else {
                return Err(NanocodeError::InvalidAttemptState {
                    detail: "completed tool call did not retain its output",
                });
            };
            let Some(active_index) = self
                .active_tool_calls
                .iter()
                .position(|candidate| Arc::ptr_eq(&candidate.completion, &active.completion))
            else {
                return Err(NanocodeError::InvalidAttemptState {
                    detail: "completed tool call was not active",
                });
            };
            self.active_tool_calls.remove(active_index);
            conversation.append(self.finish_completed_tool_call(completed, &active.progress)?);
        }
        self.finish_active_tool_batch_wall();
        Ok(())
    }

    pub(super) fn prepare_model_tool_call(
        &mut self,
        call_index: u32,
        call: &CodeCall,
    ) -> Result<ActiveToolCall> {
        self.tool_call_indices
            .insert(call.call_id.clone().into_boxed_str(), call_index);
        let qualified_name = qualified_tool_name(call);
        let arguments = if call.name == "exec" {
            ToolCallArguments::Text(&call.input)
        } else {
            serde_json::from_str::<&RawValue>(&call.input)
                .map_or(ToolCallArguments::Text(&call.input), ToolCallArguments::Raw)
        };
        self.events.emit(
            AgentEventKind::ToolCall,
            ToolCallEvent {
                call_id: &call.call_id,
                tool: &qualified_name,
                arguments,
                model_call_index: call_index,
                truncated: false,
                original_bytes: None,
            },
        )?;
        self.stats.tool_calls += 1;
        let started_at = Instant::now();
        let span = model_tool_span(&self.provider_session_id, call, call_index);
        record_span_content(&span, "tool.arguments", &call.input);
        let active = ActiveToolCall {
            call_id: call.call_id.clone(),
            name: qualified_name,
            kind: call.kind,
            started_at,
            shell_abort_format: call.namespace.is_none()
                && matches!(call.name.as_str(), "shell_command" | "unified_exec"),
            completion: Arc::default(),
            progress: Arc::default(),
            execution_started_at: Arc::default(),
            span,
        };
        self.active_tool_calls.push(active.clone());
        Ok(active)
    }

    pub(super) fn finish_completed_tool_call(
        &mut self,
        completed: CompletedToolCall,
        progress: &ArcSwap<ActiveToolProgress>,
    ) -> Result<Vec<ResponseItem>> {
        if !completed
            .cell
            .as_ref()
            .is_some_and(|cell| cell.running && cell.origin_call_id == completed.call_id)
        {
            self.tool_call_indices.remove(completed.call_id.as_str());
        }
        if let Some(cell) = &completed.cell
            && !cell.running
        {
            self.tool_call_indices.remove(cell.origin_call_id.as_str());
        }
        self.stats.tool_work_duration_ns += completed.work_duration_ns;
        let _ = self.finish_active_tool_progress(progress);
        Ok(completed.response_items)
    }

    pub(super) fn finish_active_tool_progress(
        &mut self,
        progress: &ArcSwap<ActiveToolProgress>,
    ) -> Vec<ActiveNestedToolCall> {
        let progress = Arc::unwrap_or_clone(progress.swap(Arc::default()));
        self.stats.tool_calls += progress.nested_tool_calls;
        progress.active_nested_tool_calls
    }

    pub(super) fn completed_tool_work_duration(active: &ActiveToolCall) -> u64 {
        active
            .execution_started_at
            .load()
            .as_ref()
            .map_or(0, |started_at| elapsed_ns(**started_at))
    }

    pub(super) fn finish_cancelled_tool_work(&mut self, active: &ActiveToolCall) {
        if let Some(started_at) = active.execution_started_at.swap(None) {
            self.stats.tool_work_duration_ns += elapsed_ns(*started_at);
        }
    }

    pub(super) fn finish_active_tool_batch_wall(&mut self) {
        if let Some(started_at) = self.active_tool_batch_started_at.take() {
            self.stats.tool_wall_duration_ns += elapsed_ns(started_at);
        }
    }

    pub(super) fn emit_completed_tool_result(
        events: &EventSink,
        completed: &CompletedToolCall,
    ) -> Result<()> {
        let logged = logged_top_level_result(completed);
        events.emit(
            AgentEventKind::ToolResult,
            ToolResultEvent {
                call_id: &completed.call_id,
                tool: &completed.tool,
                status: status(completed.success),
                duration_ns: completed.duration_ns,
                started_after_ns: None,
                result: logged
                    .as_ref()
                    .map_or(&completed.output, |logged| &logged.result),
                structured_result: logged
                    .as_ref()
                    .map_or(&completed.structured_result, |logged| {
                        &logged.structured_result
                    }),
                metadata: match &logged {
                    Some(logged) if !logged.metadata => None,
                    _ => completed.metadata.as_deref(),
                },
                cell: completed.cell.as_ref(),
                truncated: logged.is_some(),
                original_bytes: logged.as_ref().map(|logged| logged.original_bytes),
            },
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_cancelled_tool_result(
        &self,
        call_id: &str,
        tool: &str,
        started_at: Instant,
        started_after_ns: Option<u64>,
        shell_abort_format: bool,
        span: Option<&tracing::Span>,
        partial_output: Option<&str>,
    ) -> Result<ToolOutputBody> {
        let duration_ns = elapsed_ns(started_at);
        let elapsed_seconds = started_at.elapsed().as_secs_f64();
        let mut text = if shell_abort_format {
            format!("Wall time: {elapsed_seconds:.1} seconds\naborted by user")
        } else {
            format!("aborted by user after {:.1}s", elapsed_seconds.max(0.1))
        };
        if let Some(partial) = partial_output {
            text.push_str("; output before it stopped:\n");
            text.push_str(partial);
        }
        let output = ToolOutputBody::Text(text);
        let structured_result = output.structured_result();
        if let Some(span) = span {
            record_tool_span_terminal(span, "cancelled", "ERROR", duration_ns, &output);
        }
        self.events.emit(
            AgentEventKind::ToolResult,
            ToolResultEvent {
                call_id,
                tool,
                status: "cancelled",
                duration_ns,
                started_after_ns,
                result: &output,
                structured_result: &structured_result,
                metadata: None,
                cell: None,
                truncated: false,
                original_bytes: None,
            },
        )?;
        Ok(output)
    }

    pub(super) fn panicked_tool_call(
        active: &ActiveToolCall,
        payload: Box<dyn Any + Send>,
    ) -> CompletedToolCall {
        let message = panic_payload(payload);
        record_span_content(&active.span, "tool.panic", &message);
        let output = ToolOutputBody::Text("aborted".to_owned());
        let structured_result = output.structured_result();
        let duration_ns = elapsed_ns(active.started_at);
        record_tool_span_terminal(&active.span, "failed", "ERROR", duration_ns, &output);
        let response_item = match active.kind {
            CodeCallKind::Custom => custom_tool_output(active.call_id.clone(), output.clone()),
            CodeCallKind::Function => function_tool_output(active.call_id.clone(), output.clone()),
            CodeCallKind::ToolSearch => tool_search_output(active.call_id.clone(), Vec::new()),
        };
        CompletedToolCall {
            cell: None,
            call_id: active.call_id.clone(),
            tool: active.name.clone(),
            success: false,
            duration_ns,
            work_duration_ns: Self::completed_tool_work_duration(active),
            output,
            structured_result,
            metadata: None,
            response_items: vec![response_item],
        }
    }

    fn interrupted_tool_call(kind: CodeCallKind, call_id: &str, name: &str) -> CompletedToolCall {
        let output = ToolOutputBody::Text(
            "Tool execution interrupted by a host crash; outcome unknown. Do not assume it did not run or automatically repeat it.".into(),
        );
        let response_item = match kind {
            CodeCallKind::Custom => custom_tool_output(call_id.to_owned(), output.clone()),
            CodeCallKind::Function => function_tool_output(call_id.to_owned(), output.clone()),
            CodeCallKind::ToolSearch => tool_search_output(call_id.to_owned(), Vec::new()),
        };
        CompletedToolCall {
            cell: None,
            call_id: call_id.to_owned(),
            tool: name.to_owned(),
            success: false,
            duration_ns: 0,
            work_duration_ns: 0,
            output,
            structured_result: serde_json::json!({"outcome": "unknown", "outcome_unknown": true}),
            metadata: None,
            response_items: vec![response_item],
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_model_tool_call(
        tools: &ToolRuntime,
        images: &ImageHistory,
        events: &EventSink,
        tool_call_indices: &HashMap<Box<str>, u32>,
        call_index: u32,
        call: CodeCall,
        history: Option<Arc<SharedHistory>>,
        session_id: &str,
        turn_id: &str,
        model: Model,
        host_context: Option<&str>,
        instruction_revision: Option<u64>,
        started_at: Instant,
        progress: &ArcSwap<ActiveToolProgress>,
        tool_span: &tracing::Span,
        recover: bool,
    ) -> Result<CompletedToolCall> {
        let qualified_name = qualified_tool_name(&call);
        if let Some(message) = unsupported_tool_message(tools, &call) {
            let output = ToolOutputBody::Text(message);
            let structured_result = output.structured_result();
            record_tool_span_terminal(tool_span, "failed", "ERROR", 0, &output);
            let response_item = match call.kind {
                CodeCallKind::Custom => custom_tool_output(call.call_id.clone(), output.clone()),
                CodeCallKind::Function => {
                    function_tool_output(call.call_id.clone(), output.clone())
                }
                CodeCallKind::ToolSearch => tool_search_output(call.call_id.clone(), Vec::new()),
            };
            return Ok(CompletedToolCall {
                cell: None,
                call_id: call.call_id,
                tool: qualified_name,
                success: false,
                duration_ns: 0,
                work_duration_ns: 0,
                output,
                structured_result,
                metadata: None,
                response_items: vec![response_item],
            });
        }
        let code_mode_entrypoint =
            call.namespace.is_none() && matches!(call.name.as_str(), "exec" | "wait");
        if !code_mode_entrypoint
            && matches!(call.kind, CodeCallKind::Function | CodeCallKind::Custom)
        {
            let context = ToolContext::new(
                model.as_str(),
                session_id,
                &call.call_id,
                &[],
                DEFAULT_TOOL_OUTPUT_TOKENS,
            )
            .with_instruction_revision(instruction_revision)
            .with_host_context(host_context)
            .with_turn_id(Some(turn_id));
            let mut execution = if recover {
                let Some(recovered) = tools.recover_tool(&qualified_name, context).await else {
                    return Ok(Self::interrupted_tool_call(
                        call.kind,
                        &call.call_id,
                        &qualified_name,
                    ));
                };
                recovered
            } else if tools.is_code_only() {
                ToolOutput::error(
                    "This tool requires Code Mode; no direct handler was invoked. Reconcile any prior recovered effect before calling it through tools inside exec.",
                )
            } else {
                match call.kind {
                    CodeCallKind::Function => match RawValue::from_string(call.input.clone()) {
                        Ok(input) => tools
                            .execute_tool(&qualified_name, ToolInput::Function(input), context)
                            .instrument(tool_span.clone())
                            .await
                            .map_err(interrupted_tool_host)?,
                        Err(error) => ToolOutput::error(format!(
                            "failed to encode {qualified_name} arguments: {error}"
                        )),
                    },
                    CodeCallKind::Custom => tools
                        .execute_tool(
                            &qualified_name,
                            ToolInput::Freeform(call.input.clone()),
                            context,
                        )
                        .instrument(tool_span.clone())
                        .await
                        .map_err(interrupted_tool_host)?,
                    CodeCallKind::ToolSearch => {
                        unreachable!("tool search is not an ordinary direct tool")
                    }
                }
            };

            images
                .prepare_output_images(&mut execution.output)
                .await
                .map_err(nanocode_durability::agent_store_error)?;
            let structured_result = execution.take_structured_result();
            if let Some(content) = serialize_trace_content(&execution.output) {
                record_span_content(tool_span, "tool.output", &content);
            }
            let duration_ns = elapsed_ns(started_at);
            tool_span.record("status", status(execution.success));
            tool_span.record("otel.status_code", otel_status(execution.success));
            tool_span.record("duration_ns", duration_ns);
            return Ok(CompletedToolCall {
                cell: None,
                call_id: call.call_id.clone(),
                tool: qualified_name,
                success: execution.success,
                duration_ns,
                work_duration_ns: 0,
                response_items: vec![match call.kind {
                    CodeCallKind::Function => {
                        function_tool_output(call.call_id, execution.output.clone())
                    }
                    CodeCallKind::Custom => {
                        custom_tool_output(call.call_id, execution.output.clone())
                    }
                    CodeCallKind::ToolSearch => {
                        unreachable!("tool search is not an ordinary direct tool")
                    }
                }],
                output: execution.output,
                structured_result,
                metadata: execution.metadata,
            });
        }
        if matches!(call.kind, CodeCallKind::ToolSearch) {
            let mut context = ToolContext::new(
                model.as_str(),
                session_id,
                &call.call_id,
                &[],
                DEFAULT_TOOL_OUTPUT_TOKENS,
            );
            if let Some(history) = &history {
                context = context.with_shared_history(history);
            }
            let context = context
                .with_instruction_revision(instruction_revision)
                .with_host_context(host_context)
                .with_turn_id(Some(turn_id));
            let execution = if tools.is_code_only() {
                ToolOutput::error(
                    "Tool discovery requires Code Mode; call tools.tool_search inside exec. No direct discovery was executed.",
                )
            } else {
                match RawValue::from_string(call.input.clone()) {
                    Ok(input) => tools
                        .execute_tool("tool_search", ToolInput::Function(input), context)
                        .instrument(tool_span.clone())
                        .await
                        .map_err(interrupted_tool_host)?,
                    Err(error) => ToolOutput::error(format!(
                        "failed to encode tool_search arguments: {error}"
                    )),
                }
            };
            if let Some(content) = serialize_trace_content(&execution.output) {
                record_span_content(tool_span, "tool.output", &content);
            }
            let duration_ns = elapsed_ns(started_at);
            tool_span.record("status", status(execution.success));
            tool_span.record("otel.status_code", otel_status(execution.success));
            tool_span.record("duration_ns", duration_ns);
            let structured_result = execution.structured_result();
            let tools = if execution.success {
                match &structured_result {
                    Value::Array(tools) => tools.clone(),
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            };
            return Ok(CompletedToolCall {
                cell: None,
                call_id: call.call_id.clone(),
                tool: qualified_name,
                success: execution.success,
                duration_ns,
                work_duration_ns: 0,
                response_items: vec![tool_search_output(call.call_id, tools)],
                output: execution.output,
                structured_result,
                metadata: execution.metadata,
            });
        }
        let owned_context = if recover {
            None
        } else {
            owned_code_context(
                &call,
                history,
                session_id,
                turn_id,
                model,
                host_context,
                instruction_revision,
            )?
        };
        let context = ToolContext::new(
            model.as_str(),
            session_id,
            &call.call_id,
            &[],
            DEFAULT_TOOL_OUTPUT_TOKENS,
        )
        .with_instruction_revision(instruction_revision)
        .with_host_context(host_context)
        .with_turn_id(Some(turn_id));
        let mut observer = NestedToolEventObserver {
            events,
            tool_call_indices,
            progress,
            fallback_call_index: call_index,
            parent_call_id: &call.call_id,
            error: None,
        };
        let Some(mut execution) = execute_code_call(
            tools,
            &call,
            owned_context,
            context,
            &mut observer,
            tool_span,
            recover,
        )
        .await?
        else {
            return Ok(Self::interrupted_tool_call(
                call.kind,
                &call.call_id,
                &qualified_name,
            ));
        };
        let update_error = observer.error.take();
        drop(observer);
        if let Some(error) = update_error {
            return Err(error);
        }
        images
            .prepare_output_images(&mut execution.output)
            .await
            .map_err(nanocode_durability::agent_store_error)?;
        let structured_result = execution.output.structured_result();
        if let Some(content) = serialize_trace_content(&execution.output) {
            record_span_content(tool_span, "tool.output", &content);
        }
        let duration_ns = elapsed_ns(started_at);
        tool_span.record("status", status(execution.success));
        tool_span.record("otel.status_code", otel_status(execution.success));
        tool_span.record("duration_ns", duration_ns);
        let output = match call.kind {
            CodeCallKind::Custom => {
                custom_tool_output(call.call_id.clone(), execution.output.clone())
            }
            CodeCallKind::Function => {
                function_tool_output(call.call_id.clone(), execution.output.clone())
            }
            CodeCallKind::ToolSearch => {
                unreachable!("native tool_search returned through Code Mode")
            }
        };
        let mut outputs = Vec::with_capacity(execution.notifications.len() + 1);
        outputs.push(output);
        outputs.extend(
            execution.notifications.into_iter().map(|notification| {
                custom_tool_notification(notification.call_id, notification.text)
            }),
        );
        Ok(CompletedToolCall {
            cell: execution.cell,
            call_id: call.call_id,
            tool: qualified_name,
            success: execution.success,
            duration_ns,
            work_duration_ns: 0,
            output: execution.output,
            structured_result,
            metadata: None,
            response_items: outputs,
        })
    }
}
