use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

use serde::{Serialize, Serializer, ser::SerializeSeq};
use serde_json::value::{RawValue, to_raw_value};

use super::{ContentItem, FunctionOutputBody, FunctionOutputContent, ResponseItem};
use crate::client::{ModelConfig, Thinking, pricing::ServiceTier, responses::StrictJsonSchema};

#[derive(Clone)]
pub struct RequestProfile {
    session_id: String,
    thread_id: String,
    prompt_cache_key: String,
    prefix: Arc<[ResponseItem]>,
    code_mode_tool_names: Arc<BTreeMap<String, CodeModeToolName>>,
    tool_namespaces_info: Arc<RawValue>,
    logical_turn: u64,
    retained_config: Option<RetainedRequestConfig>,
}

#[derive(Clone)]
struct RetainedRequestConfig {
    model_id_prefix: Option<String>,
    reasoning_mode: crate::client::ReasoningMode,
    store_responses: bool,
}

impl RequestProfile {
    #[must_use]
    pub fn new(
        session_id: impl Into<String>,
        prompt_cache_key: impl Into<String>,
        prefix: Arc<[ResponseItem]>,
    ) -> Self {
        let session_id = session_id.into();
        let code_mode_tool_names = Arc::default();
        Self {
            thread_id: session_id.clone(),
            session_id,
            prompt_cache_key: prompt_cache_key.into(),
            tool_namespaces_info: tool_namespaces_info(&prefix, &code_mode_tool_names),
            prefix,
            code_mode_tool_names,
            logical_turn: 0,
            retained_config: None,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    #[must_use]
    pub fn turn_id(&self) -> String {
        format!("{}:{}", self.thread_id(), self.logical_turn)
    }

    #[must_use]
    pub fn with_thread_id(mut self, thread_id: impl Into<String>) -> Self {
        self.thread_id = thread_id.into();
        self
    }

    pub(crate) const fn with_logical_turn(mut self, logical_turn: u64) -> Self {
        self.logical_turn = logical_turn;
        self
    }

    #[must_use]
    pub fn prompt_cache_key(&self) -> &str {
        &self.prompt_cache_key
    }

    #[must_use]
    pub fn prefix(&self) -> &[ResponseItem] {
        &self.prefix
    }

    #[doc(hidden)]
    #[must_use]
    pub fn shared_prefix(&self) -> Arc<[ResponseItem]> {
        Arc::clone(&self.prefix)
    }

    pub(crate) fn with_request_content(
        mut self,
        prompt_cache_key: String,
        prefix: Arc<[ResponseItem]>,
        model_id_prefix: Option<String>,
        reasoning_mode: crate::client::ReasoningMode,
        store_responses: bool,
    ) -> Self {
        self.prompt_cache_key = prompt_cache_key;
        self.tool_namespaces_info = tool_namespaces_info(&prefix, &self.code_mode_tool_names);
        self.prefix = prefix;
        self.retained_config = Some(RetainedRequestConfig {
            model_id_prefix,
            reasoning_mode,
            store_responses,
        });
        self
    }

    pub(crate) fn with_code_mode_tool_names(
        mut self,
        names: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        self.code_mode_tool_names = Arc::new(
            names
                .into_iter()
                .map(|(identifier, name)| (identifier, CodeModeToolName::from_flat_name(name)))
                .collect(),
        );
        self.tool_namespaces_info = tool_namespaces_info(&self.prefix, &self.code_mode_tool_names);
        self
    }
}

#[derive(Serialize)]
struct CodeModeToolName {
    name: Box<str>,
    namespace: Option<Box<str>>,
}

impl CodeModeToolName {
    fn from_flat_name(name: String) -> Self {
        if let Some(namespaced) = name.strip_prefix("mcp__")
            && let Some((server, tool)) = namespaced.split_once("__")
        {
            return Self {
                name: tool.into(),
                namespace: Some(format!("mcp__{server}").into()),
            };
        }
        if let Some((namespace, tool)) = name.split_once("__") {
            return Self {
                name: tool.into(),
                namespace: Some(namespace.into()),
            };
        }
        Self {
            name: name.into(),
            namespace: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct ResponseHistory {
    head: Option<Arc<HistorySegment>>,
    tail: Arc<Vec<ResponseItem>>,
}

struct HistorySegment {
    previous: Option<Arc<Self>>,
    items: Arc<Vec<ResponseItem>>,
    len: usize,
}

impl ResponseHistory {
    #[must_use]
    pub fn new(items: Vec<ResponseItem>) -> Self {
        Self {
            head: None,
            tail: Arc::new(items),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.head.as_ref().map_or(0, |segment| segment.len) + self.tail.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn tail(&self) -> &[ResponseItem] {
        &self.tail
    }

    #[must_use]
    pub fn shared_tail(&self) -> Arc<Vec<ResponseItem>> {
        Arc::clone(&self.tail)
    }

    pub fn push(&mut self, item: ResponseItem) {
        Arc::make_mut(&mut self.tail).push(item);
    }

    pub fn tail_mut(&mut self) -> &mut Vec<ResponseItem> {
        Arc::make_mut(&mut self.tail)
    }

    pub fn commit_tail(&mut self) {
        if self.tail.is_empty() {
            return;
        }
        let items = std::mem::take(&mut self.tail);
        let previous_len = self.head.as_ref().map_or(0, |segment| segment.len);
        self.head = Some(Arc::new(HistorySegment {
            previous: self.head.take(),
            len: previous_len + items.len(),
            items,
        }));
    }

    pub fn replace(&mut self, items: Vec<ResponseItem>) {
        self.head = None;
        self.tail = Arc::new(items);
    }

    #[doc(hidden)]
    pub fn replace_suffix(&mut self, start: usize, replacement: Vec<ResponseItem>) {
        let start = start.min(self.len());
        let committed_len = self.head.as_ref().map_or(0, |segment| segment.len);
        if start >= committed_len {
            let tail_prefix_len = start - committed_len;
            let mut tail = Vec::with_capacity(tail_prefix_len + replacement.len());
            tail.extend(self.tail[..tail_prefix_len].iter().cloned());
            tail.extend(replacement);
            self.tail = Arc::new(tail);
            return;
        }
        let mut current = self.head.clone();
        while let Some(segment) = current.take() {
            let previous_len = segment.previous.as_ref().map_or(0, |previous| previous.len);
            if start >= previous_len {
                self.head.clone_from(&segment.previous);
                self.tail = Arc::new(segment.items[..start - previous_len].to_vec());
                break;
            }
            current.clone_from(&segment.previous);
        }
        Arc::make_mut(&mut self.tail).extend(replacement);
    }

    #[doc(hidden)]
    #[must_use]
    pub fn shared_prefix_len(&self, other: &Self) -> usize {
        let mut left = self.head.as_ref();
        let mut right = other.head.as_ref();
        while let (Some(segment), Some(other_segment)) = (left, right) {
            if Arc::ptr_eq(segment, other_segment) {
                return segment.len;
            }
            if segment.len >= other_segment.len {
                left = segment.previous.as_ref();
            } else {
                right = other_segment.previous.as_ref();
            }
        }
        0
    }

    #[must_use]
    pub fn iter(&self) -> ResponseHistoryIter<'_> {
        ResponseHistoryIter::new(self, 0)
    }

    #[must_use]
    pub fn iter_from(&self, start: usize) -> ResponseHistoryIter<'_> {
        ResponseHistoryIter::new(self, start)
    }

    #[must_use]
    pub fn iter_rev(&self) -> ResponseHistoryRevIter<'_> {
        ResponseHistoryRevIter {
            tail: self.tail.iter().rev(),
            segment: self.head.as_deref(),
            segment_items: None,
            remaining: self.len(),
        }
    }
}

impl<'a> IntoIterator for &'a ResponseHistory {
    type Item = &'a ResponseItem;
    type IntoIter = ResponseHistoryIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct ResponseHistoryIter<'a> {
    segments: Vec<&'a HistorySegment>,
    segment_index: usize,
    item_index: usize,
    tail: std::slice::Iter<'a, ResponseItem>,
}

impl<'a> ResponseHistoryIter<'a> {
    fn new(history: &'a ResponseHistory, start: usize) -> Self {
        let mut segments = Vec::new();
        let committed_len = history.head.as_ref().map_or(0, |segment| segment.len);
        let start = start.min(history.len());
        let mut item_index = 0;
        if start < committed_len {
            let mut current = history.head.as_deref();
            while let Some(segment) = current {
                let previous_len = segment.previous.as_ref().map_or(0, |previous| previous.len);
                segments.push(segment);
                if start >= previous_len {
                    item_index = start - previous_len;
                    break;
                }
                current = segment.previous.as_deref();
            }
            segments.reverse();
        }
        let tail_start = start.saturating_sub(committed_len);
        Self {
            segments,
            segment_index: 0,
            item_index,
            tail: history.tail[tail_start..].iter(),
        }
    }
}

impl<'a> Iterator for ResponseHistoryIter<'a> {
    type Item = &'a ResponseItem;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(segment) = self.segments.get(self.segment_index) {
            if let Some(item) = segment.items.get(self.item_index) {
                self.item_index += 1;
                return Some(item);
            }
            self.segment_index += 1;
            self.item_index = 0;
        }
        self.tail.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self
            .segments
            .iter()
            .enumerate()
            .skip(self.segment_index)
            .map(|(index, segment)| {
                if index == self.segment_index {
                    segment.items.len().saturating_sub(self.item_index)
                } else {
                    segment.items.len()
                }
            })
            .sum::<usize>()
            + self.tail.len();
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for ResponseHistoryIter<'_> {}

pub struct ResponseHistoryRevIter<'a> {
    tail: std::iter::Rev<std::slice::Iter<'a, ResponseItem>>,
    segment: Option<&'a HistorySegment>,
    segment_items: Option<std::iter::Rev<std::slice::Iter<'a, ResponseItem>>>,
    remaining: usize,
}

impl<'a> Iterator for ResponseHistoryRevIter<'a> {
    type Item = &'a ResponseItem;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(item) = self.tail.next() {
            self.remaining -= 1;
            return Some(item);
        }
        loop {
            if let Some(item) = self.segment_items.as_mut().and_then(Iterator::next) {
                self.remaining -= 1;
                return Some(item);
            }
            let segment = self.segment.take()?;
            self.segment = segment.previous.as_deref();
            self.segment_items = Some(segment.items.iter().rev());
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for ResponseHistoryRevIter<'_> {}

#[derive(Clone, Copy)]
pub struct ResponsesInput<'a> {
    first: &'a [ResponseItem],
    second: &'a [ResponseItem],
    history: Option<&'a ResponseHistory>,
    history_start: usize,
    tail: Option<&'a ResponseItem>,
}

impl<'a> ResponsesInput<'a> {
    #[must_use]
    pub const fn new(
        first: &'a [ResponseItem],
        second: &'a [ResponseItem],
        tail: Option<&'a ResponseItem>,
    ) -> Self {
        Self {
            first,
            second,
            history: None,
            history_start: 0,
            tail,
        }
    }

    #[must_use]
    pub const fn history(
        first: &'a [ResponseItem],
        history: &'a ResponseHistory,
        tail: Option<&'a ResponseItem>,
    ) -> Self {
        Self {
            first,
            second: &[],
            history: Some(history),
            history_start: 0,
            tail,
        }
    }

    #[must_use]
    pub const fn history_suffix(
        first: &'a [ResponseItem],
        history: &'a ResponseHistory,
        history_start: usize,
        tail: Option<&'a ResponseItem>,
    ) -> Self {
        Self {
            first,
            second: &[],
            history: Some(history),
            history_start,
            tail,
        }
    }

    #[must_use]
    pub fn iter(self) -> ResponsesInputIter<'a> {
        ResponsesInputIter {
            first: self.first.iter(),
            second: self.second.iter(),
            history: self
                .history
                .map(|history| history.iter_from(self.history_start)),
            tail: self.tail.into_iter(),
        }
    }

    #[must_use]
    pub fn len(self) -> usize {
        self.first.len()
            + self.second.len()
            + self.history.map_or(0, |history| {
                history.len().saturating_sub(self.history_start)
            })
            + usize::from(self.tail.is_some())
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

pub struct ResponsesInputIter<'a> {
    first: std::slice::Iter<'a, ResponseItem>,
    second: std::slice::Iter<'a, ResponseItem>,
    history: Option<ResponseHistoryIter<'a>>,
    tail: std::option::IntoIter<&'a ResponseItem>,
}

impl<'a> Iterator for ResponsesInputIter<'a> {
    type Item = &'a ResponseItem;

    fn next(&mut self) -> Option<Self::Item> {
        self.first
            .next()
            .or_else(|| self.second.next())
            .or_else(|| self.history.as_mut().and_then(Iterator::next))
            .or_else(|| self.tail.next())
    }
}

impl Serialize for ResponsesInput<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.len()))?;
        for item in self.iter() {
            sequence.serialize_element(&RequestResponseItem {
                item,
                strip_image_detail: false,
                custom_tools_as_functions: false,
            })?;
        }
        sequence.end()
    }
}

#[derive(Clone, Copy)]
struct RequestInput<'a> {
    input: ResponsesInput<'a>,
    strip_image_detail: bool,
    reasoning_effort_updates: bool,
    custom_tools_as_functions: bool,
}

impl Serialize for RequestInput<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let supported = |item: &&ResponseItem| {
            self.reasoning_effort_updates
                || !matches!(item, ResponseItem::ConfigurationUpdate { .. })
        };
        let mut sequence =
            serializer.serialize_seq(Some(self.input.iter().filter(supported).count()))?;
        for item in self.input.iter().filter(supported) {
            sequence.serialize_element(&RequestResponseItem {
                item,
                strip_image_detail: self.strip_image_detail,
                custom_tools_as_functions: self.custom_tools_as_functions,
            })?;
        }
        sequence.end()
    }
}

struct RequestResponseItem<'a> {
    item: &'a ResponseItem,
    strip_image_detail: bool,
    custom_tools_as_functions: bool,
}

impl Serialize for RequestResponseItem<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let function = self
            .custom_tools_as_functions
            .then(|| crate::client::transport::freeform::function_item(self.item))
            .flatten();
        let source = function.as_ref().unwrap_or(self.item);
        let unprefixed_id = source.id().is_some_and(|id| !id.is_prefixed());
        let strip_image_detail = self.strip_image_detail && has_image_detail(source);
        if unprefixed_id || strip_image_detail {
            let mut item = function.unwrap_or_else(|| self.item.clone());
            if unprefixed_id {
                item.set_id(None);
            }
            if strip_image_detail {
                item.strip_image_details();
            }
            item.serialize(serializer)
        } else {
            source.serialize(serializer)
        }
    }
}

fn has_image_detail(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { content, .. } => content.iter().any(|part| {
            matches!(
                part,
                ContentItem::InputImage {
                    detail: Some(_),
                    ..
                } | ContentItem::InputImageFile {
                    detail: Some(_),
                    ..
                }
            )
        }),
        ResponseItem::FunctionCallOutput {
            output: FunctionOutputBody::Content(content),
            ..
        }
        | ResponseItem::CustomToolCallOutput {
            output: FunctionOutputBody::Content(content),
            ..
        } => content.iter().any(|part| {
            matches!(
                part,
                FunctionOutputContent::InputImage {
                    detail: Some(_),
                    ..
                } | FunctionOutputContent::InputImageFile {
                    detail: Some(_),
                    ..
                }
            )
        }),
        _ => false,
    }
}

#[derive(Serialize)]
pub(crate) struct ResponseCreate<'a> {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
    model: Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<&'a str>,
    input: RequestInput<'a>,
    tool_choice: &'static str,
    parallel_tool_calls: bool,
    reasoning: ReasoningControls,
    store: bool,
    stream: bool,
    include: [&'static str; 1],
    prompt_cache_key: &'a str,
    text: TextControls<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_tier: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generate: Option<bool>,
    client_metadata: ClientMetadata<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'a serde_json::Value>,
    #[serde(rename = "providerOptions", skip_serializing_if = "Option::is_none")]
    provider_options: Option<crate::client::transport::gateway::VercelProviderOptions<'a>>,
}

impl<'a> ResponseCreate<'a> {
    #[must_use]
    pub(crate) fn warmup(
        config: &'a ModelConfig,
        model: crate::client::Model,
        thinking: Thinking,
        service_tier: ServiceTier,
        profile: &'a RequestProfile,
        turn_state: Option<&'a str>,
    ) -> Self {
        Self::new(
            config,
            CreatePolicy {
                transport: config.responses_transport,
                model,
                thinking,
                service_tier,
            },
            ResponsesInput::new(profile.prefix(), &[], None),
            None,
            Some(false),
            profile,
            turn_state,
        )
    }

    pub(crate) fn generation_with_policy(
        config: &'a ModelConfig,
        policy: CreatePolicy,
        input: ResponsesInput<'a>,
        previous_response_id: Option<&'a str>,
        profile: &'a RequestProfile,
        turn_state: Option<&'a str>,
    ) -> Self {
        Self::new(
            config,
            policy,
            input,
            previous_response_id,
            None,
            profile,
            turn_state,
        )
    }

    fn new(
        config: &'a ModelConfig,
        policy: CreatePolicy,
        input: ResponsesInput<'a>,
        previous_response_id: Option<&'a str>,
        generate: Option<bool>,
        profile: &'a RequestProfile,
        turn_state: Option<&'a str>,
    ) -> Self {
        let websocket = matches!(
            policy.transport,
            crate::client::ResponsesTransport::WebSocket
        );
        let retained = profile.retained_config.as_ref();
        let model = retained.map_or_else(
            || config.wire_model_id(policy.model),
            |retained| {
                crate::client::openai::prefixed_model_id(
                    retained.model_id_prefix.as_deref(),
                    policy.model,
                )
            },
        );
        let reasoning_mode =
            retained.map_or(config.reasoning_mode, |retained| retained.reasoning_mode);
        let store_responses =
            retained.map_or(config.store_responses, |retained| retained.store_responses);
        Self {
            kind: websocket.then_some("response.create"),
            model,
            previous_response_id,
            input: RequestInput {
                input,
                reasoning_effort_updates: policy.model.supports_reasoning_effort_updates()
                    && retained.map_or(config.model_id_prefix.is_none(), |retained| {
                        retained.model_id_prefix.is_none()
                    }),
                custom_tools_as_functions: config
                    .responses_gateway
                    .as_deref()
                    .is_some_and(|gateway| gateway.sends_custom_tools_as_functions(policy.model)),
                strip_image_detail: matches!(
                    policy.model,
                    crate::client::Model::Sol
                        | crate::client::Model::Terra
                        | crate::client::Model::Luna
                        | crate::client::Model::Astra
                        | crate::client::Model::DaybreakBlue
                ),
            },
            tool_choice: "auto",

            parallel_tool_calls: false,
            reasoning: ReasoningControls {
                mode: policy
                    .model
                    .supports_reasoning_mode(reasoning_mode)
                    .then(|| reasoning_mode.request_value())
                    .flatten(),
                effort: policy.thinking.as_str(),
                summary: "auto",
                context: "all_turns",
            },
            store: store_responses,
            stream: true,
            include: ["reasoning.encrypted_content"],
            prompt_cache_key: profile.prompt_cache_key(),
            text: TextControls {
                verbosity: "low",
                format: config.strict_json_schema.as_ref(),
            },

            service_tier: policy.service_tier.request_value(policy.model),
            generate,
            client_metadata: ClientMetadata {
                session_id: profile.session_id(),
                thread_id: profile.thread_id(),
                responses_lite: websocket.then_some("true"),
                turn_state: websocket.then_some(turn_state).flatten(),
                turn_metadata: Some(SerializedTurnMetadata {
                    profile,
                    request_kind: if generate == Some(false) {
                        "prewarm"
                    } else {
                        "turn"
                    },
                }),
            },
            provider: config
                .responses_gateway
                .as_deref()
                .and_then(crate::client::transport::ResponsesGateway::openrouter_provider),
            provider_options: config
                .responses_gateway
                .as_deref()
                .and_then(crate::client::transport::ResponsesGateway::vercel_provider_options),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CreatePolicy {
    transport: crate::client::ResponsesTransport,
    model: crate::client::Model,
    thinking: Thinking,
    service_tier: ServiceTier,
}

impl CreatePolicy {
    pub(crate) const fn new(
        transport: crate::client::ResponsesTransport,
        model: crate::client::Model,
        thinking: Thinking,
        service_tier: ServiceTier,
    ) -> Self {
        Self {
            transport,
            model,
            thinking,
            service_tier,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
struct ReasoningControls {
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<&'static str>,
    effort: &'static str,
    summary: &'static str,
    context: &'static str,
}

#[derive(Clone, Copy, Serialize)]
struct TextControls<'a> {
    verbosity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a StrictJsonSchema>,
}

#[derive(Clone, Copy, Serialize)]
struct ClientMetadata<'a> {
    session_id: &'a str,
    thread_id: &'a str,
    #[serde(rename = "ws_request_header_x_openai_internal_codex_responses_lite")]
    #[serde(skip_serializing_if = "Option::is_none")]
    responses_lite: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-codex-turn-state")]
    turn_state: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-codex-turn-metadata")]
    turn_metadata: Option<SerializedTurnMetadata<'a>>,
}

#[derive(Clone, Copy)]
struct SerializedTurnMetadata<'a> {
    profile: &'a RequestProfile,
    request_kind: &'static str,
}

impl Serialize for SerializedTurnMetadata<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct TurnMetadata<'a> {
            session_id: &'a str,
            thread_id: &'a str,
            turn_id: String,
            request_kind: &'static str,
            tool_namespaces_info: &'a RawValue,
        }

        let profile = self.profile;
        let value = serde_json::to_string(&TurnMetadata {
            session_id: profile.session_id(),
            thread_id: profile.thread_id(),
            turn_id: profile.turn_id(),
            request_kind: self.request_kind,
            tool_namespaces_info: &profile.tool_namespaces_info,
        })
        .map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&value)
    }
}

fn tool_namespaces_info(
    prefix: &[ResponseItem],
    code_mode_tool_names: &BTreeMap<String, CodeModeToolName>,
) -> Arc<RawValue> {
    use super::ToolDefinition;
    use serde_json::json;
    fn insert(
        result: &mut BTreeMap<String, serde_json::Value>,
        namespace: &str,
        name: &str,
        direct: bool,
        code_name: Option<&str>,
        deferred: bool,
    ) {
        let source = namespace.strip_prefix("mcp__").map_or_else(
            || json!({"kind":"harness"}),
            |server| json!({"kind":"mcp", "server_name":server}),
        );
        let entry = result
            .entry(namespace.to_owned())
            .or_insert_with(|| json!({"name":namespace,"functions":{}}));
        let function = &mut entry["functions"][name];
        if function.is_null() {
            *function = json!({"name":name,"direct":direct,"code_mode_name":code_name,"deferred":deferred,"source":source});
        } else {
            if direct {
                function["direct"] = json!(true);
            }
            if let Some(code_name) = code_name {
                function["code_mode_name"] = json!(code_name);
            }
        }
    }
    fn direct(
        result: &mut BTreeMap<String, serde_json::Value>,
        namespace: &str,
        definition: &ToolDefinition,
    ) {
        match definition {
            ToolDefinition::Namespace { name, tools, .. } => {
                for tool in tools {
                    direct(result, name, tool);
                }
            }
            ToolDefinition::Function {
                name,
                defer_loading,
                ..
            }
            | ToolDefinition::Custom {
                name,
                defer_loading,
                ..
            } => insert(
                result,
                namespace,
                name,
                true,
                None,
                defer_loading.unwrap_or(false),
            ),
            ToolDefinition::ToolSearch { .. } => {
                insert(result, "tool_search", "tool_search_tool", true, None, false)
            }
        }
    }
    let mut result = BTreeMap::new();
    for item in prefix {
        if let ResponseItem::AdditionalTools { tools, .. } = item {
            for tool in tools {
                direct(&mut result, "functions", tool);
            }
        }
    }
    for (identifier, tool) in code_mode_tool_names {
        insert(
            &mut result,
            tool.namespace.as_deref().unwrap_or("functions"),
            &tool.name,
            false,
            Some(identifier),
            false,
        );
    }
    Arc::from(to_raw_value(&result).expect("JSON values serialize"))
}
