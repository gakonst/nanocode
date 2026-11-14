use std::{borrow::Cow, collections::HashMap};

use crate::client::{
    AgentEventKind, ContentItem, EventSink, MessagePhase, MessageRole, ResponseEvent, ResponseItem,
    ResponseItemId, events::TextDeltaRef, monotonic_now_ns, responses::Usage,
};
use nanocode_platform::Instant;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::client::{
    ResponsesError,
    attempt::ResponsesObserver,
    service_error::ResponsesServiceError,
    socket::{ReceivedText, ResponsesSocket},
    telemetry::{ApiEvent, ApiEventData, elapsed_ns},
    transport::{error::Credentials, freeform::FreeformCalls},
};

#[derive(Deserialize, Serialize)]
pub struct GenerationOutput {
    pub id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_model: Option<String>,

    pub status: String,

    pub end_turn: Option<bool>,

    pub final_message: Option<String>,

    pub output_items: Vec<ResponseItem>,

    #[serde(default)]
    pub code_calls: Vec<CodeCall>,

    pub usage: Option<Usage>,

    pub time_to_first_event_ns: u64,

    pub time_to_first_output_ns: Option<u64>,

    #[serde(default)]
    pub pipeline_stats: ResponsePipelineStats,

    #[serde(default)]
    pub timing: ResponseTiming,
}

impl GenerationOutput {
    #[doc(hidden)]
    pub fn restore_derived_fields(&mut self) {
        self.code_calls = code_calls(&self.output_items);
        self.final_message = final_message(&self.output_items);
    }
}

pub struct CompactionOutput {
    pub id: String,

    pub status: String,

    pub item: ResponseItem,

    pub usage: Option<Usage>,

    pub time_to_first_event_ns: u64,

    pub time_to_first_output_ns: Option<u64>,

    pub pipeline_stats: ResponsePipelineStats,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ResponseTiming {
    pub queue_ns: Option<u64>,

    pub attempt_started_ns: Option<u64>,

    pub connect_ns: Option<u64>,

    pub encode_ns: Option<u64>,

    pub send_ns: Option<u64>,

    pub request_sent_ns: Option<u64>,

    pub response_headers_ns: Option<u64>,

    pub first_event_ns: Option<u64>,

    pub first_output_ns: Option<u64>,

    pub response_completed_ns: Option<u64>,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ResponsePipelineStats {
    pub event_count: u64,

    pub event_bytes: u64,

    pub receive_wait_duration_ns: u64,

    pub parse_duration_ns: u64,

    pub emit_duration_ns: u64,

    pub decode_duration_ns: u64,

    pub response_forward_duration_ns: u64,

    pub display_delta_emit_duration_ns: u64,

    pub socket_queue_duration_ns: u64,

    pub display_delta_count: u64,

    pub display_delta_bytes: u64,

    pub inter_delta_gap_duration_ns: u64,

    pub inter_delta_gap_max_ns: u64,

    pub inter_delta_stall_50ms_count: u64,

    pub inter_delta_stall_100ms_count: u64,

    pub inter_delta_stall_250ms_count: u64,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CodeCall {
    pub call_id: String,

    pub name: String,

    pub namespace: Option<String>,

    pub input: String,

    pub kind: CodeCallKind,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeCallKind {
    Custom,

    Function,

    ToolSearch,
}

#[derive(Serialize)]
struct AssistantMessage<'a> {
    model_call_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<MessagePhase>,
    text: Cow<'a, str>,
}

struct AssistantStreamItem {
    item_id: Option<ResponseItemId>,
    phase: Option<MessagePhase>,
}

struct StreamTiming {
    started_at: Instant,
    model_started_at: Option<Instant>,
    model_started_ns: Option<u64>,
    first_event_ns: Option<u64>,
    first_output_ns: Option<u64>,
    first_event_model_ns: Option<u64>,
    first_output_model_ns: Option<u64>,
    pipeline: ResponsePipelineStats,
    last_display_delta_received_ns: Option<u64>,
}

impl StreamTiming {
    fn new(started_at: Instant, model_started_at: Option<Instant>) -> Self {
        Self {
            started_at,
            model_started_at,
            model_started_ns: model_started_at
                .map(|started_at| monotonic_now_ns().saturating_sub(elapsed_ns(started_at))),
            first_event_ns: None,
            first_output_ns: None,
            first_event_model_ns: None,
            first_output_model_ns: None,
            pipeline: ResponsePipelineStats {
                event_count: 0,
                event_bytes: 0,
                receive_wait_duration_ns: 0,
                parse_duration_ns: 0,
                emit_duration_ns: 0,
                decode_duration_ns: 0,
                response_forward_duration_ns: 0,
                display_delta_emit_duration_ns: 0,
                socket_queue_duration_ns: 0,
                display_delta_count: 0,
                display_delta_bytes: 0,
                inter_delta_gap_duration_ns: 0,
                inter_delta_gap_max_ns: 0,
                inter_delta_stall_50ms_count: 0,
                inter_delta_stall_100ms_count: 0,
                inter_delta_stall_250ms_count: 0,
            },
            last_display_delta_received_ns: None,
        }
    }

    fn model_elapsed_ns(&self) -> Option<u64> {
        self.model_started_at.map(elapsed_ns)
    }

    fn record_display_delta(&mut self, received_ns: u64, bytes: usize) {
        self.pipeline.display_delta_count = self.pipeline.display_delta_count.saturating_add(1);
        self.pipeline.display_delta_bytes = self
            .pipeline
            .display_delta_bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        if let Some(previous_ns) = self.last_display_delta_received_ns {
            let gap_ns = received_ns.saturating_sub(previous_ns);
            self.pipeline.inter_delta_gap_duration_ns = self
                .pipeline
                .inter_delta_gap_duration_ns
                .saturating_add(gap_ns);
            self.pipeline.inter_delta_gap_max_ns = self.pipeline.inter_delta_gap_max_ns.max(gap_ns);
            self.pipeline.inter_delta_stall_50ms_count = self
                .pipeline
                .inter_delta_stall_50ms_count
                .saturating_add(u64::from(gap_ns >= 50_000_000));
            self.pipeline.inter_delta_stall_100ms_count = self
                .pipeline
                .inter_delta_stall_100ms_count
                .saturating_add(u64::from(gap_ns >= 100_000_000));
            self.pipeline.inter_delta_stall_250ms_count = self
                .pipeline
                .inter_delta_stall_250ms_count
                .saturating_add(u64::from(gap_ns >= 250_000_000));
        }
        self.last_display_delta_received_ns = Some(received_ns);
    }

    fn record_first_output(&mut self) {
        if self.first_output_ns.is_none() {
            self.first_output_ns = Some(elapsed_ns(self.started_at));
            self.first_output_model_ns = self.model_elapsed_ns();
        }
    }
}

pub(crate) trait ResponseEventSource {
    async fn next_text(&mut self) -> Result<ReceivedText, ResponsesError>;

    fn try_next_text(&mut self) -> Result<Option<ReceivedText>, ResponsesError>;
}

impl ResponseEventSource for ResponsesSocket {
    async fn next_text(&mut self) -> Result<ReceivedText, ResponsesError> {
        Self::next_text(self).await
    }

    fn try_next_text(&mut self) -> Result<Option<ReceivedText>, ResponsesError> {
        Self::try_next_text(self)
    }
}

struct FreeformEvents<'a, S> {
    source: &'a mut S,
    calls: Option<FreeformCalls>,
}

impl<S: ResponseEventSource> FreeformEvents<'_, S> {
    fn rewrite(&mut self, mut received: ReceivedText) -> ReceivedText {
        if let Some(text) = self
            .calls
            .as_mut()
            .and_then(|calls| calls.rewrite(received.text.as_str()))
        {
            #[cfg(not(target_family = "wasm"))]
            {
                received.text = text.into();
            }
            #[cfg(target_family = "wasm")]
            {
                received.text = text;
            }
        }
        received
    }
}

impl<S: ResponseEventSource> ResponseEventSource for FreeformEvents<'_, S> {
    async fn next_text(&mut self) -> Result<ReceivedText, ResponsesError> {
        let received = self.source.next_text().await?;
        Ok(self.rewrite(received))
    }

    fn try_next_text(&mut self) -> Result<Option<ReceivedText>, ResponsesError> {
        let received = self.source.try_next_text()?;
        Ok(received.map(|received| self.rewrite(received)))
    }
}

#[derive(Deserialize)]
struct WireEvent<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    #[serde(borrow)]
    output_index: Option<&'a RawValue>,
    #[serde(borrow)]
    delta: Option<&'a RawValue>,
    #[serde(borrow)]
    item: Option<&'a RawValue>,
    #[serde(borrow)]
    response: Option<&'a RawValue>,
    #[serde(borrow)]
    item_id: Option<&'a RawValue>,
    #[serde(borrow)]
    call_id: Option<&'a RawValue>,
    #[serde(borrow)]
    text: Option<&'a RawValue>,
    #[serde(borrow)]
    summary_index: Option<&'a RawValue>,
    #[serde(borrow)]
    content_index: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct Text<'a>(#[serde(borrow)] Cow<'a, str>);

#[derive(Deserialize)]
struct CompletedResponse<'a> {
    id: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default = "completed_status")]
    status: String,
    #[serde(default)]
    end_turn: Option<bool>,
    #[serde(default, borrow)]
    output: Option<&'a RawValue>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct OutputItemHeader<'a> {
    #[serde(default, borrow)]
    id: Option<Cow<'a, str>>,
}

fn completed_status() -> String {
    "completed".to_owned()
}

enum StreamEvent<'a> {
    ItemAdded {
        output_index: Option<u32>,
        item: ResponseItem,
    },
    TextDelta {
        output_index: Option<u32>,
        delta: Cow<'a, str>,
    },
    ReasoningDelta {
        delta: Cow<'a, str>,
    },
    ToolInputDelta {
        output_index: Option<u32>,
        item_id: Option<Cow<'a, str>>,
        delta: Cow<'a, str>,
    },
    ItemDone {
        output_index: Option<u32>,
        item: ResponseItem,
    },
    Completed(CompletedResponse<'a>),
    Failure,
    Other,
}

impl StreamEvent<'_> {
    const fn is_output(&self) -> bool {
        matches!(
            self,
            Self::TextDelta { .. }
                | Self::ReasoningDelta { .. }
                | Self::ToolInputDelta { .. }
                | Self::ItemAdded { .. }
                | Self::ItemDone { .. }
        )
    }
}

type DecodedEvent<'a> = (StreamEvent<'a>, Option<ResponseEvent>);

fn required<'a, T: Deserialize<'a>>(
    value: Option<&'a RawValue>,
    field: &'static str,
) -> Result<T, serde_json::Error> {
    let value =
        value.ok_or_else(|| <serde_json::Error as serde::de::Error>::missing_field(field))?;
    serde_json::from_str(value.get())
}

fn optional<'a, T: Deserialize<'a>>(
    value: Option<&'a RawValue>,
) -> Result<Option<T>, serde_json::Error> {
    value
        .map(|value| serde_json::from_str(value.get()))
        .transpose()
}

fn decode_wire_event(raw: &str, responses: bool) -> Result<DecodedEvent<'_>, serde_json::Error> {
    let event = serde_json::from_str::<WireEvent<'_>>(raw)?;
    Ok(match event.kind.as_ref() {
        "response.output_text.delta" => {
            let delta = required::<Text<'_>>(event.delta, "delta")?.0;
            let response =
                responses.then(|| ResponseEvent::OutputTextDelta(delta.clone().into_owned()));
            (
                StreamEvent::TextDelta {
                    output_index: optional(event.output_index)?,
                    delta,
                },
                response,
            )
        }
        "response.reasoning_summary_text.delta" => {
            let delta = required::<Text<'_>>(event.delta, "delta")?.0;
            let summary_index = optional(event.summary_index)?.unwrap_or(0);
            let response = responses.then(|| ResponseEvent::ReasoningSummaryDelta {
                delta: delta.clone().into_owned(),
                summary_index,
            });
            (StreamEvent::ReasoningDelta { delta }, response)
        }
        "response.reasoning_summary.delta" => {
            let delta = required::<Text<'_>>(event.delta, "delta")?.0;
            let summary_index = optional(event.summary_index)?;
            let response = summary_index.filter(|_| responses).map(|summary_index| {
                ResponseEvent::ReasoningSummaryDelta {
                    delta: delta.clone().into_owned(),
                    summary_index,
                }
            });
            (StreamEvent::ReasoningDelta { delta }, response)
        }
        "response.reasoning_text.delta" => {
            let delta = required::<Text<'_>>(event.delta, "delta")?.0;
            let content_index = optional(event.content_index)?.unwrap_or(0);
            let response = responses.then(|| ResponseEvent::ReasoningContentDelta {
                delta: delta.clone().into_owned(),
                content_index,
            });
            (StreamEvent::ReasoningDelta { delta }, response)
        }
        "response.output_item.added" => {
            let item = required::<ResponseItem>(event.item, "item")?;
            let response = responses.then(|| ResponseEvent::OutputItemAdded(item.clone()));
            (
                StreamEvent::ItemAdded {
                    output_index: optional(event.output_index)?,
                    item,
                },
                response,
            )
        }
        "response.output_item.done" => {
            let item = required::<ResponseItem>(event.item, "item")?;
            let response = responses.then(|| ResponseEvent::OutputItemDone(item.clone()));
            (
                StreamEvent::ItemDone {
                    output_index: optional(event.output_index)?,
                    item,
                },
                response,
            )
        }
        "response.completed" => {
            let completed = required::<CompletedResponse<'_>>(event.response, "response")?;
            let response = responses.then(|| ResponseEvent::Completed {
                usage: completed.usage.clone(),
                end_turn: completed.end_turn,
            });
            (StreamEvent::Completed(completed), response)
        }
        "response.function_call_arguments.delta" => (
            StreamEvent::ToolInputDelta {
                output_index: optional(event.output_index)?,
                item_id: optional::<Text<'_>>(event.item_id)?.map(|item| item.0),
                delta: required::<Text<'_>>(event.delta, "delta")?.0,
            },
            None,
        ),
        "response.custom_tool_call_input.delta" => {
            let delta = required::<Text<'_>>(event.delta, "delta")?.0;
            let item_id = optional::<Text<'_>>(event.item_id)?.map(|item| item.0);
            let call_id = optional::<String>(event.call_id)?;
            let response = responses
                .then(|| {
                    Some(ResponseEvent::ToolCallInputDelta {
                        item_id: item_id
                            .as_deref()
                            .map(str::to_owned)
                            .or_else(|| call_id.clone())?,
                        call_id,
                        delta: delta.clone().into_owned(),
                    })
                })
                .flatten();
            (
                StreamEvent::ToolInputDelta {
                    output_index: optional(event.output_index)?,
                    item_id,
                    delta,
                },
                response,
            )
        }
        "response.reasoning_summary_text.done" => {
            let item_id = required::<String>(event.item_id, "item_id")?;
            let text = required::<Text<'_>>(event.text, "text")?.0;
            let summary_index = optional(event.summary_index)?.unwrap_or(0);
            let response = responses.then(|| ResponseEvent::ReasoningSummaryDone {
                item_id,
                text: text.into_owned(),
                summary_index,
            });
            (StreamEvent::Other, response)
        }
        "response.reasoning_summary_part.added" => {
            let summary_index = optional(event.summary_index)?.unwrap_or(0);
            let response =
                responses.then_some(ResponseEvent::ReasoningSummaryPartAdded { summary_index });
            (StreamEvent::Other, response)
        }
        "response.created" => (
            StreamEvent::Other,
            responses.then_some(ResponseEvent::Created),
        ),
        "response.failed" | "response.incomplete" | "error" => (StreamEvent::Failure, None),
        _ => (StreamEvent::Other, None),
    })
}

struct ReceivedEvent<'a> {
    raw: &'a str,
    decoded: Result<DecodedEvent<'a>, ResponsesError>,
    decoded_ns: u64,
}

fn decode_received<'a>(
    received: &'a ReceivedText,
    started_ns: u64,
    timing: &mut StreamTiming,
    responses: bool,
) -> Result<ReceivedEvent<'a>, ResponsesError> {
    let text = received.text.as_str();
    timing.pipeline.event_count = timing.pipeline.event_count.saturating_add(1);
    timing.pipeline.socket_queue_duration_ns = timing
        .pipeline
        .socket_queue_duration_ns
        .saturating_add(started_ns.saturating_sub(received.received_ns));
    timing.pipeline.event_bytes = timing
        .pipeline
        .event_bytes
        .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
    let raw = text.trim_matches([' ', '\t', '\n', '\r']);
    let decoded = match decode_wire_event(raw, responses) {
        Ok(decoded) => Ok(decoded),
        Err(source) if source.is_syntax() || source.is_eof() => {
            return Err(ResponsesError::InvalidJson(source));
        }
        Err(source) => Err(ResponsesError::InvalidPayload {
            source,
            event: raw.to_owned(),
        }),
    };
    let decoded_ns = monotonic_now_ns();
    timing.pipeline.decode_duration_ns = timing
        .pipeline
        .decode_duration_ns
        .saturating_add(decoded_ns.saturating_sub(started_ns));
    if timing.first_event_ns.is_none() {
        timing.first_event_ns = Some(elapsed_ns(timing.started_at));
        timing.first_event_model_ns = timing.model_elapsed_ns();
    }
    Ok(ReceivedEvent {
        raw,
        decoded,
        decoded_ns,
    })
}

struct EventContext<'a> {
    transport: &'static str,
    observer: &'a ResponsesObserver,
    credentials: &'a Credentials,
    phase: &'static str,
    call_index: u32,
}

#[derive(Clone, Copy)]
struct Published {
    api_event_seq: u64,
    ready_ns: u64,
}

async fn publish_received<'a>(
    context: &EventContext<'_>,
    received: &ReceivedText,
    event: ReceivedEvent<'a>,
    timing: &mut StreamTiming,
) -> Result<(StreamEvent<'a>, Published), ResponsesServiceError> {
    let failure = matches!(&event.decoded, Ok((StreamEvent::Failure, _)));
    let traced = nanocode_trace::enabled!(target: "nanocode_codex", nanocode_trace::Level::TRACE);
    let raw_event = if failure || traced || context.observer.raw_api_events {
        context.credentials.redact(event.raw)
    } else {
        Cow::Borrowed(event.raw)
    };
    if traced {
        nanocode_trace::trace!(
            target: "nanocode_codex",
            direction = "inbound",
            transport = context.transport,
            phase = context.phase,
            model.call_index = context.call_index,
            api.event = %raw_event,
            "OpenAI Responses API event"
        );
    }
    let api_event_seq = context.observer.events.emit_with_source_sequence(
        AgentEventKind::ApiEvent,
        ApiEvent {
            direction: "inbound",
            transport: context.transport,
            phase: context.phase,
            model_call_index: Some(context.call_index),
            event: ApiEventData::Received(&raw_event),
        },
        Some(received.received_ns),
    )?;
    let emitted_ns = monotonic_now_ns();
    timing.pipeline.emit_duration_ns = timing
        .pipeline
        .emit_duration_ns
        .saturating_add(emitted_ns.saturating_sub(event.decoded_ns));
    let (stream_event, response) = event.decoded?;
    if failure {
        return Err(
            ResponsesServiceError::from(ResponsesError::api_event_received(
                raw_event.into_owned(),
                received.retry_receipt,
            ))
            .with_connection_renewal(timing.pipeline.event_count == 1),
        );
    }
    let mut ready_ns = emitted_ns;
    if let Some(response) = response {
        context.observer.emit_response(response).await;
        ready_ns = monotonic_now_ns();
        timing.pipeline.response_forward_duration_ns = timing
            .pipeline
            .response_forward_duration_ns
            .saturating_add(ready_ns.saturating_sub(emitted_ns));
    }
    if stream_event.is_output() {
        timing.record_first_output();
    }
    Ok((
        stream_event,
        Published {
            api_event_seq,
            ready_ns,
        },
    ))
}

#[allow(
    clippy::too_many_arguments,
    reason = "each argument is a distinct input of the generation stream"
)]
pub(crate) async fn receive<S>(
    source: &mut S,
    freeform: Option<FreeformCalls>,
    transport: &'static str,
    observer: &ResponsesObserver,
    credentials: &Credentials,
    call_index: u32,
    started_at: Instant,
    model_started_at: Option<Instant>,
) -> Result<GenerationOutput, ResponsesServiceError>
where
    S: ResponseEventSource,
{
    let source = &mut FreeformEvents {
        source,
        calls: freeform,
    };
    let mut done_items = Vec::with_capacity(2);
    let mut assistant_items = HashMap::new();
    let mut tool_items = HashMap::new();
    let item_namespace = nanocode_platform::uuid_v4();
    let mut timing = StreamTiming::new(started_at, model_started_at);
    let context = EventContext {
        transport,
        observer,
        credentials,
        phase: "generation",
        call_index,
    };
    let responses = observer.wants_responses();
    let events = &observer.events;

    loop {
        let (received, started_ns) = match source.try_next_text() {
            Ok(Some(received)) => (received, monotonic_now_ns()),
            Ok(None) => {
                flush_display_deltas(events, &mut timing)?;
                let wait_started_ns = monotonic_now_ns();
                let received = source.next_text().await?;
                let started_ns = monotonic_now_ns();
                timing.pipeline.receive_wait_duration_ns = timing
                    .pipeline
                    .receive_wait_duration_ns
                    .saturating_add(started_ns.saturating_sub(wait_started_ns));
                (received, started_ns)
            }
            Err(error) => {
                flush_display_deltas(events, &mut timing)?;
                return Err(error.into());
            }
        };
        let event = match decode_received(&received, started_ns, &mut timing, responses) {
            Ok(event) => event,
            Err(error) => {
                flush_display_deltas(events, &mut timing)?;
                return Err(error.into());
            }
        };
        let (event, published) =
            match publish_received(&context, &received, event, &mut timing).await {
                Ok(published) => published,
                Err(error) => {
                    flush_display_deltas(events, &mut timing)?;
                    return Err(error);
                }
            };
        match event {
            StreamEvent::ItemAdded {
                output_index,
                mut item,
            } => {
                let Some(output_index) = output_index else {
                    continue;
                };
                normalize_stream_item(
                    &mut item,
                    output_index,
                    &item_namespace,
                    &mut assistant_items,
                );
                if let Some((tool, input)) = ToolStreamItem::of(&item) {
                    if !observer.tool_input_deltas {
                        continue;
                    }
                    if !input.is_empty() {
                        emit_tool_input(
                            events,
                            &mut timing,
                            call_index,
                            &tool,
                            Cow::Borrowed(input),
                            &received,
                            published,
                        )?;
                    }
                    tool_items.insert(output_index, tool);
                    continue;
                }
                let ResponseItem::Message {
                    id,
                    role: MessageRole::Assistant,
                    phase,
                    ..
                } = item
                else {
                    continue;
                };
                assistant_items.insert(output_index, AssistantStreamItem { item_id: id, phase });
            }
            StreamEvent::TextDelta {
                output_index,
                delta,
            } => {
                let index = output_index.unwrap_or(0);
                let item = &*assistant_items
                    .entry(index)
                    .or_insert_with(|| AssistantStreamItem {
                        item_id: Some(ResponseItemId::with_suffix(
                            "msg",
                            nanocode_platform::Uuid::new_v5(&item_namespace, &index.to_be_bytes()),
                        )),
                        phase: None,
                    });
                emit_display_delta(
                    events,
                    &mut timing,
                    AgentEventKind::AssistantDelta,
                    TextDeltaRef {
                        model_call_index: call_index,
                        item_id: item.item_id.as_deref().map(Cow::Borrowed),
                        call_id: None,
                        tool: None,
                        phase: item.phase,
                        text: delta,
                        model_elapsed_ns: None,
                    },
                    &received,
                    published,
                )?;
            }
            StreamEvent::ReasoningDelta { delta } => {
                emit_display_delta(
                    events,
                    &mut timing,
                    AgentEventKind::ReasoningSummaryDelta,
                    TextDeltaRef {
                        model_call_index: call_index,
                        item_id: None,
                        call_id: None,
                        tool: None,
                        phase: None,
                        text: delta,
                        model_elapsed_ns: None,
                    },
                    &received,
                    published,
                )?;
            }
            StreamEvent::ToolInputDelta {
                output_index,
                item_id,
                delta,
            } => {
                let tool = output_index
                    .and_then(|index| tool_items.get(&index))
                    .or_else(|| {
                        let item_id = item_id.as_deref()?;
                        tool_items
                            .values()
                            .find(|tool| tool.item_id.as_deref() == Some(item_id))
                    });
                if let Some(tool) = tool {
                    emit_tool_input(
                        events,
                        &mut timing,
                        call_index,
                        tool,
                        delta,
                        &received,
                        published,
                    )?;
                }
            }
            StreamEvent::ItemDone {
                output_index,
                mut item,
            } => {
                normalize_stream_item(
                    &mut item,
                    output_index.unwrap_or(done_items.len() as u32),
                    &item_namespace,
                    &mut assistant_items,
                );
                emit_assistant_message(events, call_index, &item)?;
                done_items.push(item);
            }
            StreamEvent::Completed(response) => {
                let emitted_ids = done_items
                    .iter()
                    .filter_map(|item| item.id().cloned())
                    .collect::<std::collections::HashSet<_>>();
                let mut output_items = completed_output(response.output, done_items)?;
                for (index, item) in output_items.iter_mut().enumerate() {
                    normalize_stream_item(
                        item,
                        index as u32,
                        &item_namespace,
                        &mut assistant_items,
                    );
                    if item.id().is_some_and(|id| !emitted_ids.contains(id)) {
                        emit_assistant_message(events, call_index, item)?;
                    }
                }
                let code_calls = code_calls(&output_items);
                let final_message = final_message(&output_items);
                return Ok(GenerationOutput {
                    id: response.id,
                    reported_model: response.model,
                    status: response.status,
                    end_turn: response.end_turn,
                    final_message,
                    output_items,
                    code_calls,
                    usage: response.usage,
                    time_to_first_event_ns: timing.first_event_ns.unwrap_or_default(),
                    time_to_first_output_ns: timing.first_output_ns,
                    pipeline_stats: timing.pipeline,
                    timing: ResponseTiming {
                        first_event_ns: timing.first_event_model_ns,
                        first_output_ns: timing.first_output_model_ns,
                        response_completed_ns: timing.model_elapsed_ns(),
                        ..ResponseTiming::default()
                    },
                });
            }
            StreamEvent::Failure | StreamEvent::Other => {}
        }
    }
}

fn completed_output(
    output: Option<&RawValue>,
    done_items: Vec<ResponseItem>,
) -> Result<Vec<ResponseItem>, ResponsesError> {
    let Some(output) = output else {
        return Ok(done_items);
    };
    let invalid = |source| ResponsesError::InvalidPayload {
        source,
        event: output.get().to_owned(),
    };
    let headers =
        serde_json::from_str::<Vec<OutputItemHeader<'_>>>(output.get()).map_err(invalid)?;
    let streamed = headers.len() == done_items.len()
        && headers.iter().zip(&done_items).all(|(header, item)| {
            header
                .id
                .as_deref()
                .is_some_and(|id| item.id().is_some_and(|item_id| item_id.as_str() == id))
        });
    if headers.is_empty() || streamed {
        return Ok(done_items);
    }
    serde_json::from_str(output.get()).map_err(invalid)
}

struct ToolStreamItem {
    item_id: Option<String>,
    call_id: String,
    tool: String,
}

impl ToolStreamItem {
    fn of(item: &ResponseItem) -> Option<(Self, &str)> {
        let (id, call_id, name, namespace, input) = match item {
            ResponseItem::FunctionCall {
                id,
                call_id,
                name,
                namespace,
                arguments,
                ..
            } => (id, call_id, name, namespace, arguments),
            ResponseItem::CustomToolCall {
                id,
                call_id,
                name,
                namespace,
                input,
                ..
            } => (id, call_id, name, namespace, input),
            _ => return None,
        };
        Some((
            Self {
                item_id: id.as_deref().map(str::to_owned),
                call_id: call_id.to_string(),
                tool: qualified_tool_name(name, namespace.as_deref()),
            },
            input,
        ))
    }
}

#[must_use]
pub fn qualified_tool_name(name: &str, namespace: Option<&str>) -> String {
    let Some(namespace) = namespace else {
        return name.to_owned();
    };
    if namespace.ends_with('_') || name.starts_with('_') {
        format!("{namespace}{name}")
    } else {
        format!("{namespace}__{name}")
    }
}

fn emit_tool_input(
    events: &EventSink,
    timing: &mut StreamTiming,
    call_index: u32,
    tool: &ToolStreamItem,
    text: Cow<'_, str>,
    received: &ReceivedText,
    published: Published,
) -> Result<(), ResponsesServiceError> {
    emit_display_delta(
        events,
        timing,
        AgentEventKind::ToolInputDelta,
        TextDeltaRef {
            model_call_index: call_index,
            item_id: None,
            call_id: Some(Cow::Borrowed(&tool.call_id)),
            tool: Some(Cow::Borrowed(&tool.tool)),
            phase: None,
            text,
            model_elapsed_ns: None,
        },
        received,
        published,
    )
}

fn emit_display_delta(
    events: &EventSink,
    timing: &mut StreamTiming,
    kind: AgentEventKind,
    mut delta: TextDeltaRef<'_>,
    received: &ReceivedText,
    published: Published,
) -> Result<(), ResponsesServiceError> {
    let payload_bytes = delta.text.len();
    timing.record_display_delta(received.received_ns, payload_bytes);
    delta.model_elapsed_ns = timing
        .model_started_ns
        .map(|started_ns| published.ready_ns.saturating_sub(started_ns));
    events.emit_text_delta(kind, delta, Some(received.received_ns))?;
    let emitted_ns = monotonic_now_ns();
    timing.pipeline.display_delta_emit_duration_ns = timing
        .pipeline
        .display_delta_emit_duration_ns
        .saturating_add(emitted_ns.saturating_sub(published.ready_ns));
    nanocode_trace::trace!(
        target: "nanocode_stream_timing",
        stage = "api_delta_emitted",
        request.id = events.request_id(),
        event.kind = ?kind,
        source.api.event.seq = published.api_event_seq,
        payload.bytes = payload_bytes,
        socket_to_agent_emit_ns = emitted_ns.saturating_sub(received.received_ns),
        "Responses display delta entered the agent event stream"
    );
    Ok(())
}

fn flush_display_deltas(
    events: &EventSink,
    timing: &mut StreamTiming,
) -> Result<(), ResponsesServiceError> {
    let started_ns = monotonic_now_ns();
    events.flush_deltas()?;
    timing.pipeline.display_delta_emit_duration_ns = timing
        .pipeline
        .display_delta_emit_duration_ns
        .saturating_add(monotonic_now_ns().saturating_sub(started_ns));
    Ok(())
}

fn normalize_stream_item(
    item: &mut ResponseItem,
    index: u32,
    namespace: &nanocode_platform::Uuid,
    known: &mut HashMap<u32, AssistantStreamItem>,
) {
    if let ResponseItem::Message {
        id,
        role: MessageRole::Assistant,
        phase,
        ..
    } = item
    {
        let stream = known.entry(index).or_insert_with(|| AssistantStreamItem {
            item_id: id.clone().or_else(|| {
                Some(ResponseItemId::with_suffix(
                    "msg",
                    nanocode_platform::Uuid::new_v5(namespace, &index.to_be_bytes()),
                ))
            }),
            phase: *phase,
        });
        *id = stream.item_id.clone();
        if phase.is_some() {
            stream.phase = *phase;
        } else {
            *phase = stream.phase;
        }
    }
}

fn emit_assistant_message(
    events: &EventSink,
    call_index: u32,
    item: &ResponseItem,
) -> Result<(), ResponsesServiceError> {
    let ResponseItem::Message {
        id,
        role: MessageRole::Assistant,
        content,
        phase,
        ..
    } = item
    else {
        return Ok(());
    };
    events.emit(
        AgentEventKind::AssistantMessage,
        AssistantMessage {
            model_call_index: call_index,
            item_id: id.as_deref(),
            phase: *phase,
            text: output_text(content),
        },
    )?;
    Ok(())
}

pub(crate) async fn receive_compaction<S>(
    source: &mut S,
    local: bool,
    transport: &'static str,
    observer: &ResponsesObserver,
    credentials: &Credentials,
    call_index: u32,
    started_at: Instant,
) -> Result<CompactionOutput, ResponsesServiceError>
where
    S: ResponseEventSource,
{
    let mut done_items = Vec::with_capacity(2);
    let mut timing = StreamTiming::new(started_at, None);
    let context = EventContext {
        transport,
        observer,
        credentials,
        phase: "compaction",
        call_index,
    };
    let responses = observer.wants_responses();

    loop {
        let wait_started_ns = monotonic_now_ns();
        let received = source.next_text().await?;
        let started_ns = monotonic_now_ns();
        timing.pipeline.receive_wait_duration_ns = timing
            .pipeline
            .receive_wait_duration_ns
            .saturating_add(started_ns.saturating_sub(wait_started_ns));
        let event = decode_received(&received, started_ns, &mut timing, responses)?;
        match publish_received(&context, &received, event, &mut timing)
            .await?
            .0
        {
            StreamEvent::ItemDone { item, .. } => done_items.push(item),
            StreamEvent::Completed(response) => {
                let item = if local {
                    crate::client::session::compaction::summary_message(last_output_text(
                        &done_items,
                    ))
                } else {
                    let mut compactions = done_items
                        .into_iter()
                        .filter(|item| matches!(item, ResponseItem::Compaction { .. }));
                    let item = compactions.next();
                    let count = usize::from(item.is_some()) + compactions.count();
                    if count != 1 {
                        return Err(ResponsesServiceError::invalid_compaction(count));
                    }
                    let Some(item) = item else {
                        return Err(ResponsesServiceError::invalid_compaction(0));
                    };
                    item
                };
                return Ok(CompactionOutput {
                    id: response.id,
                    status: response.status,
                    item,
                    usage: response.usage,
                    time_to_first_event_ns: timing.first_event_ns.unwrap_or_default(),
                    time_to_first_output_ns: timing.first_output_ns,
                    pipeline_stats: timing.pipeline,
                });
            }
            _ => {}
        }
    }
}

fn code_calls(items: &[ResponseItem]) -> Vec<CodeCall> {
    let mut calls = Vec::with_capacity(items.len().min(4));
    for item in items {
        match item {
            ResponseItem::CustomToolCall {
                call_id,
                name,
                namespace,
                input,
                ..
            } => {
                calls.push(CodeCall {
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                    namespace: namespace.as_deref().map(str::to_owned),
                    input: input.to_string(),
                    kind: CodeCallKind::Custom,
                });
            }
            ResponseItem::FunctionCall {
                call_id,
                name,
                namespace,
                arguments,
                ..
            } => {
                calls.push(CodeCall {
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                    namespace: namespace.as_deref().map(str::to_owned),
                    input: arguments.to_string(),
                    kind: CodeCallKind::Function,
                });
            }
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                execution,
                arguments,
                ..
            } if execution.as_ref() == "client" => {
                calls.push(CodeCall {
                    call_id: call_id.to_string(),
                    name: "tool_search".to_owned(),
                    namespace: None,
                    input: arguments.as_value().to_string(),
                    kind: CodeCallKind::ToolSearch,
                });
            }
            _ => {}
        }
    }
    calls
}

fn last_output_text(items: &[ResponseItem]) -> &str {
    items
        .iter()
        .rev()
        .find_map(|item| {
            let ResponseItem::Message {
                role: MessageRole::Assistant,
                content,
                ..
            } = item
            else {
                return None;
            };
            content.iter().rev().find_map(|part| match part {
                ContentItem::OutputText { text, .. } => Some(text.as_ref()),
                ContentItem::InputText { .. }
                | ContentItem::InputImage { .. }
                | ContentItem::InputImageFile { .. }
                | ContentItem::InputAudio { .. } => None,
            })
        })
        .unwrap_or_default()
}

fn final_message(items: &[ResponseItem]) -> Option<String> {
    items.iter().rev().find_map(|item| {
        let ResponseItem::Message {
            role: MessageRole::Assistant,
            content,
            ..
        } = item
        else {
            return None;
        };
        Some(output_text(content).into_owned())
    })
}

fn output_text(content: &[ContentItem]) -> Cow<'_, str> {
    let mut parts = content.iter().filter_map(|part| match part {
        ContentItem::OutputText { text, .. } => Some(text.as_ref()),
        ContentItem::InputText { .. }
        | ContentItem::InputImage { .. }
        | ContentItem::InputImageFile { .. }
        | ContentItem::InputAudio { .. } => None,
    });
    let Some(first) = parts.next() else {
        return Cow::Borrowed("");
    };
    match parts.next() {
        None => Cow::Borrowed(first),
        Some(second) => {
            let mut text = String::from(first);
            text.push_str(second);
            text.extend(parts);
            Cow::Owned(text)
        }
    }
}
