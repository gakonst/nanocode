use std::collections::HashSet;

use crate::client::{
    ContentItem, FunctionOutputBody, FunctionOutputContent, MessageRole, ResponseItem,
    ResponseItemId, Usage, responses::ResponseHistory,
};

use super::compaction;

pub const TOOL_OUTPUT_TOKEN_LIMIT: usize = 12_000;
const REQUEST_PREFIX_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x3e203f80_1cd8_4938_9588_0990bb023db5);

const SYNTHETIC_OUTPUT_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x90d38d3e_6a5b_4d52_bfe2_2f1e634bfac4);

#[derive(Clone)]
pub struct ContextManager {
    items: ResponseHistory,
    pub(super) last_token_usage: Option<Usage>,
    pub(super) token_usage_is_estimate: bool,
    calls: CallIds,
}

#[derive(Clone, Default)]
struct CallIds {
    function_calls: HashSet<Box<str>>,
    function_outputs: HashSet<Box<str>>,
    custom_calls: HashSet<Box<str>>,
    custom_outputs: HashSet<Box<str>>,
    tool_search_calls: HashSet<Box<str>>,
    tool_search_outputs: HashSet<Box<str>>,
    non_server_tool_search_outputs: HashSet<Box<str>>,
}

use crate::client::tools::valid_tool_image_data_url;

impl ContextManager {
    #[must_use]
    pub fn new(items: Vec<ResponseItem>) -> Self {
        let mut context = Self {
            items: ResponseHistory::default(),
            last_token_usage: None,
            token_usage_is_estimate: false,
            calls: CallIds::default(),
        };
        context.record_items(items);
        context
    }

    #[must_use]
    pub fn flattened_items(&self) -> Vec<ResponseItem> {
        self.items.iter().cloned().collect()
    }

    #[must_use]
    pub fn shared_items(&self) -> ResponseHistory {
        self.items.clone()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &ResponseItem> {
        self.items.iter()
    }

    pub fn record_items(&mut self, items: impl IntoIterator<Item = ResponseItem>) {
        for mut item in items
            .into_iter()
            .filter(is_api_item)
            .map(truncate_tool_output)
        {
            if let ResponseItem::CustomToolCallOutput {
                call_id,
                name: Some(_),
                ..
            } = &item
                && !self.items.iter().any(|item| {
                    matches!(
                        item,
                        ResponseItem::CustomToolCall {
                            call_id: candidate,
                            ..
                        } if candidate == call_id
                    )
                })
            {
                continue;
            }
            assign_missing_response_item_id(&mut item);
            self.calls.track(&item);
            if self.token_usage_is_estimate
                && let Some(usage) = &mut self.last_token_usage
            {
                usage.total_tokens = usage
                    .total_tokens
                    .saturating_add(compaction::estimate_item_tokens(&item));
            }
            self.items.push(item);
        }
    }

    pub fn commit_tail(&mut self) {
        self.items.commit_tail();
        if self.calls.is_balanced() {
            self.calls.clear();
        }
    }

    pub(super) fn replace_invalid_tool_images(&mut self) -> usize {
        let mut replaced = 0;
        let mut items = self.flattened_items();
        for item in &mut items {
            let (ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. }) = item
            else {
                continue;
            };
            let FunctionOutputBody::Content(content) = output else {
                continue;
            };
            for part in content {
                let invalid = match part {
                    FunctionOutputContent::InputImage { image_url, .. } => {
                        !valid_tool_image_data_url(image_url)
                            && !image_url.starts_with("nanocode-blob:")
                    }
                    FunctionOutputContent::InputImageFile { file_id, .. } => {
                        !crate::client::responses::valid_image_file_id(file_id)
                    }
                    _ => false,
                };
                if invalid {
                    let text = if matches!(part, FunctionOutputContent::InputImageFile { .. }) {
                        "[image omitted: invalid file ID in restored tool output]"
                    } else {
                        "[image omitted: malformed base64 image data in restored tool output]"
                    };
                    *part = FunctionOutputContent::InputText { text: text.into() };
                    replaced += 1;
                }
            }
        }
        if replaced > 0 {
            self.replace_and_recompute(items, &[]);
        }
        replaced
    }

    pub fn replace_rejected_images(&mut self) -> usize {
        let mut replaced = 0;
        let mut items = self.flattened_items();
        for item in &mut items {
            match item {
                ResponseItem::Message { content, .. } => {
                    for content in content {
                        if matches!(
                            content,
                            ContentItem::InputImage { .. } | ContentItem::InputImageFile { .. }
                        ) {
                            *content = ContentItem::input_text(
                                "[image omitted after the provider rejected its data]",
                            );
                            replaced += 1;
                        }
                    }
                }
                ResponseItem::FunctionCallOutput { output, .. }
                | ResponseItem::CustomToolCallOutput { output, .. } => {
                    let FunctionOutputBody::Content(content) = output else {
                        continue;
                    };
                    for content in content {
                        if matches!(
                            content,
                            FunctionOutputContent::InputImage { .. }
                                | FunctionOutputContent::InputImageFile { .. }
                        ) {
                            *content = FunctionOutputContent::InputText {
                                text: "[image omitted after the provider rejected its data]".into(),
                            };
                            replaced += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        if replaced > 0 {
            self.replace_and_recompute(items, &[]);
        }
        replaced
    }

    pub fn remove_tool_definition(&mut self, definition: &serde_json::Value) -> usize {
        let mut removed = 0;
        let mut items = self.flattened_items();
        for item in &mut items {
            if let ResponseItem::ToolSearchOutput { tools, .. } = item {
                tools.retain_mut(|tool| {
                    let mut value = tool.as_value().clone();
                    let before = removed;
                    let retain = retain_tool_definition(&mut value, definition, &mut removed);
                    if retain && removed != before {
                        *tool = value.into();
                    }
                    retain
                });
            }
        }
        if removed > 0 {
            self.replace_and_recompute(items, &[]);
        }
        removed
    }

    pub fn replace_and_recompute(&mut self, mut items: Vec<ResponseItem>, prefix: &[ResponseItem]) {
        assign_missing_response_item_ids(&mut items);
        self.items.replace(items);
        let total_tokens = prefix
            .iter()
            .chain(self.items.iter())
            .map(compaction::estimate_item_tokens)
            .fold(0, u64::saturating_add);
        self.last_token_usage = Some(Usage {
            total_tokens,
            ..Usage::default()
        });
        self.token_usage_is_estimate = true;
        self.calls = CallIds::from_items(self.items.iter());
    }

    pub fn update_token_info(&mut self, usage: Option<&Usage>) {
        if let Some(usage) = usage {
            self.last_token_usage = Some(usage.clone());
            self.token_usage_is_estimate = false;
        }
    }

    #[must_use]
    pub fn active_context_tokens(&self, server_reasoning_included: bool) -> u64 {
        let reported = self
            .last_token_usage
            .as_ref()
            .map_or(0, |usage| usage.total_tokens);

        if self.token_usage_is_estimate {
            return reported;
        }
        let local_tail = self.items_after_last_model_generated_tokens();
        if server_reasoning_included {
            reported.saturating_add(local_tail)
        } else {
            reported
                .saturating_add(self.non_last_reasoning_tokens())
                .saturating_add(local_tail)
        }
    }

    #[must_use]
    pub fn prompt_items(&self) -> ResponseHistory {
        self.prompt_items_with_repair().0
    }

    pub(crate) fn prompt_items_with_repair(&self) -> (ResponseHistory, bool) {
        let needs_repair = !self.calls.is_balanced();
        if !needs_repair {
            return (self.items.clone(), false);
        }

        let calls = CallIds::from_items(self.items.iter());
        let mut repaired = Vec::with_capacity(self.items.len() + 2);
        for item in &self.items {
            match item {
                ResponseItem::FunctionCall { id, call_id, .. }
                | ResponseItem::LocalShellCall {
                    id,
                    call_id: Some(call_id),
                    ..
                } => {
                    repaired.push(item.clone());
                    if !calls.function_outputs.contains(call_id.as_ref()) {
                        let mut output = ResponseItem::function_call_output(
                            call_id.to_string(),
                            FunctionOutputBody::Text("aborted".into()),
                        );
                        output.set_id(synthetic_output_id("fco", id.as_ref()));
                        repaired.push(output);
                    }
                }
                ResponseItem::CustomToolCall { id, call_id, .. } => {
                    repaired.push(item.clone());
                    if !calls.custom_outputs.contains(call_id.as_ref()) {
                        let mut output = ResponseItem::custom_tool_output(
                            call_id.to_string(),
                            None,
                            FunctionOutputBody::Text("aborted".into()),
                        );
                        output.set_id(synthetic_output_id("ctco", id.as_ref()));
                        repaired.push(output);
                    }
                }
                ResponseItem::FunctionCallOutput { call_id, .. }
                    if !calls.function_calls.contains(call_id.as_ref()) => {}
                ResponseItem::CustomToolCallOutput { call_id, .. }
                    if !calls.custom_calls.contains(call_id.as_ref()) => {}
                ResponseItem::ToolSearchCall {
                    id,
                    call_id: Some(call_id),
                    ..
                } => {
                    repaired.push(item.clone());
                    if !calls.tool_search_outputs.contains(call_id.as_ref()) {
                        repaired.push(ResponseItem::ToolSearchOutput {
                            id: synthetic_output_id("tso", id.as_ref()),
                            call_id: Some(call_id.clone()),
                            status: "completed".into(),
                            execution: "client".into(),
                            tools: Vec::new(),
                            internal_chat_message_metadata_passthrough: None,
                        });
                    }
                }
                ResponseItem::ToolSearchOutput {
                    call_id: Some(call_id),
                    execution,
                    ..
                } if execution.as_ref() != "server"
                    && !calls.tool_search_calls.contains(call_id.as_ref()) => {}
                _ => repaired.push(item.clone()),
            }
        }
        (ResponseHistory::new(repaired), true)
    }

    pub(crate) fn adopt_prompt_items(&mut self, items: ResponseHistory) {
        if self.token_usage_is_estimate
            && let Some(usage) = &mut self.last_token_usage
        {
            let before = self
                .items
                .iter()
                .map(compaction::estimate_item_tokens)
                .fold(0, u64::saturating_add);
            let after = items
                .iter()
                .map(compaction::estimate_item_tokens)
                .fold(0, u64::saturating_add);

            usage.total_tokens = usage
                .total_tokens
                .saturating_sub(before)
                .saturating_add(after);
        }
        self.items = items;
        self.calls = CallIds::from_items(self.items.iter());
    }

    fn items_after_last_model_generated_tokens(&self) -> u64 {
        let mut tokens = None::<u64>;
        for item in &self.items {
            if is_model_generated_item(item) {
                tokens = Some(0);
            } else if let Some(tokens) = &mut tokens {
                *tokens = tokens.saturating_add(compaction::estimate_item_tokens(item));
            }
        }
        tokens.unwrap_or_default()
    }

    fn non_last_reasoning_tokens(&self) -> u64 {
        let mut reasoning = 0_u64;
        let mut before_last_user = None;
        for item in &self.items {
            if is_user_turn_boundary(item) {
                before_last_user = Some(reasoning);
            }
            if matches!(
                item,
                ResponseItem::Reasoning {
                    encrypted_content: Some(_),
                    ..
                }
            ) {
                reasoning = reasoning.saturating_add(compaction::estimate_item_tokens(item));
            }
        }
        before_last_user.unwrap_or_default()
    }
}

impl CallIds {
    fn from_items<'a>(items: impl IntoIterator<Item = &'a ResponseItem>) -> Self {
        let mut calls = Self::default();
        for item in items {
            calls.track(item);
        }
        calls
    }

    fn is_balanced(&self) -> bool {
        self.function_calls == self.function_outputs
            && self.custom_calls == self.custom_outputs
            && self.tool_search_calls.is_subset(&self.tool_search_outputs)
            && self
                .non_server_tool_search_outputs
                .is_subset(&self.tool_search_calls)
    }

    fn clear(&mut self) {
        self.function_calls.clear();
        self.function_outputs.clear();
        self.custom_calls.clear();
        self.custom_outputs.clear();
        self.tool_search_calls.clear();
        self.tool_search_outputs.clear();
        self.non_server_tool_search_outputs.clear();
    }

    fn track(&mut self, item: &ResponseItem) {
        match item {
            ResponseItem::FunctionCall { call_id, .. }
            | ResponseItem::LocalShellCall {
                call_id: Some(call_id),
                ..
            } => {
                self.function_calls.insert(call_id.clone());
            }
            ResponseItem::FunctionCallOutput { call_id, .. } => {
                self.function_outputs.insert(call_id.clone());
            }
            ResponseItem::CustomToolCall { call_id, .. } => {
                self.custom_calls.insert(call_id.clone());
            }
            ResponseItem::CustomToolCallOutput {
                call_id,
                name: None,
                ..
            } => {
                self.custom_outputs.insert(call_id.clone());
            }

            ResponseItem::CustomToolCallOutput { .. } => {}
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                ..
            } => {
                self.tool_search_calls.insert(call_id.clone());
            }
            ResponseItem::ToolSearchOutput {
                call_id: Some(call_id),
                execution,
                ..
            } => {
                self.tool_search_outputs.insert(call_id.clone());
                if execution.as_ref() != "server" {
                    self.non_server_tool_search_outputs.insert(call_id.clone());
                }
            }
            _ => {}
        }
    }
}

pub fn assign_missing_response_item_ids(items: &mut [ResponseItem]) {
    for item in items {
        assign_missing_response_item_id(item);
    }
}

pub fn assign_missing_response_item_id(item: &mut ResponseItem) {
    if item.id().is_some_and(|id| !id.is_empty()) {
        return;
    }
    let Some(prefix) = item.id_prefix() else {
        return;
    };
    item.set_id(Some(new_response_item_id(prefix)));
}

struct RequestPrefix {
    cache_lineage: Box<str>,
    tools: Vec<crate::client::ToolDefinition>,
    instructions: Box<str>,
    items: [ResponseItem; 2],
}

pub fn responses_lite_request_prefix(
    cache_lineage: &str,
    tools: Vec<crate::client::ToolDefinition>,
    instructions: &str,
) -> Result<[ResponseItem; 2], serde_json::Error> {
    static LAST: arc_swap::ArcSwapOption<RequestPrefix> = arc_swap::ArcSwapOption::const_empty();
    if let Some(last) = LAST.load().as_deref()
        && *last.cache_lineage == *cache_lineage
        && *last.instructions == *instructions
        && last.tools == tools
    {
        return Ok(last.items.clone());
    }
    let items = build_request_prefix(cache_lineage, tools.clone(), instructions)?;
    LAST.store(Some(std::sync::Arc::new(RequestPrefix {
        cache_lineage: cache_lineage.into(),
        tools,
        instructions: instructions.into(),
        items: items.clone(),
    })));
    Ok(items)
}

fn build_request_prefix(
    cache_lineage: &str,
    tools: Vec<crate::client::ToolDefinition>,
    instructions: &str,
) -> Result<[ResponseItem; 2], serde_json::Error> {
    let prefix_namespace =
        uuid::Uuid::new_v5(&REQUEST_PREFIX_ID_NAMESPACE, cache_lineage.as_bytes());
    let tools_id = crate::client::ResponseItemId::with_suffix(
        "at",
        uuid::Uuid::new_v5(&prefix_namespace, &serde_json::to_vec(&tools)?),
    );
    let instructions_id = crate::client::ResponseItemId::with_suffix(
        "msg",
        uuid::Uuid::new_v5(&prefix_namespace, instructions.as_bytes()),
    );
    let mut tools_item = ResponseItem::additional_tools(tools);
    tools_item.set_id(Some(tools_id));
    let mut instructions_item = ResponseItem::message(
        crate::client::MessageRole::Developer,
        [crate::client::ContentItem::InputText {
            text: instructions.into(),
        }],
    );
    instructions_item.set_id(Some(instructions_id));
    Ok([tools_item, instructions_item])
}

fn retain_tool_definition(
    tool: &mut serde_json::Value,
    rejected: &serde_json::Value,
    removed: &mut usize,
) -> bool {
    if tool == rejected {
        *removed += 1;
        return false;
    }
    if tool["type"] == "namespace"
        && let Some(children) = tool
            .get_mut("tools")
            .and_then(serde_json::Value::as_array_mut)
    {
        let before = *removed;
        children.retain_mut(|child| retain_tool_definition(child, rejected, removed));
        if *removed != before && children.is_empty() {
            return false;
        }
    }
    true
}

fn new_response_item_id(prefix: &str) -> ResponseItemId {
    ResponseItemId::with_suffix(prefix, nanocode_platform::uuid_v7())
}

fn synthetic_output_id(prefix: &str, source_id: Option<&ResponseItemId>) -> Option<ResponseItemId> {
    let source_id = source_id.filter(|id| !id.is_empty())?;
    let name = format!("{prefix}:{}", source_id.as_str());
    Some(ResponseItemId::with_suffix(
        prefix,
        uuid::Uuid::new_v5(&SYNTHETIC_OUTPUT_ID_NAMESPACE, name.as_bytes()),
    ))
}

#[must_use]
pub fn has_well_formed_tool_calls(items: &[ResponseItem]) -> bool {
    let mut function_calls = HashSet::new();
    let mut function_outputs = HashSet::new();
    let mut custom_calls = HashSet::new();
    let mut custom_outputs = HashSet::new();
    let mut search_calls = HashSet::new();
    let mut search_outputs = HashSet::new();
    let mut non_server_search_outputs = HashSet::new();
    for item in items {
        let valid = match item {
            ResponseItem::FunctionCall { call_id, .. }
            | ResponseItem::LocalShellCall {
                call_id: Some(call_id),
                ..
            } => function_calls.insert(call_id.as_ref()),
            ResponseItem::FunctionCallOutput { call_id, .. } => {
                function_calls.contains(call_id.as_ref())
                    && function_outputs.insert(call_id.as_ref())
            }
            ResponseItem::CustomToolCall { call_id, .. } => custom_calls.insert(call_id.as_ref()),
            ResponseItem::CustomToolCallOutput { call_id, name, .. } => {
                custom_calls.contains(call_id.as_ref())
                    && (name.is_some() || custom_outputs.insert(call_id.as_ref()))
            }
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                ..
            } => search_calls.insert(call_id.as_ref()),
            ResponseItem::ToolSearchOutput {
                call_id: Some(call_id),
                execution,
                ..
            } => {
                search_outputs.insert(call_id.as_ref());
                execution.as_ref() == "server" || non_server_search_outputs.insert(call_id.as_ref())
            }
            ResponseItem::ToolSearchCall { .. } | ResponseItem::ToolSearchOutput { .. } => true,
            _ => true,
        };
        if !valid {
            return false;
        }
    }
    function_calls == function_outputs
        && custom_calls == custom_outputs
        && search_calls.is_subset(&search_outputs)
        && non_server_search_outputs.is_subset(&search_calls)
}

const fn is_model_generated_item(item: &ResponseItem) -> bool {
    matches!(
        item,
        ResponseItem::Message {
            role: MessageRole::Assistant,
            ..
        } | ResponseItem::Reasoning { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::FunctionCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::ContextCompaction { .. }
    )
}

fn is_user_turn_boundary(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::AgentMessage { .. } => true,
        ResponseItem::Message {
            role: MessageRole::Assistant,
            content,
            ..
        } => is_inter_agent_instruction_content(content),
        _ => item.is_user_message() && !is_contextual_user_message(item),
    }
}

fn is_inter_agent_instruction_content(content: &[ContentItem]) -> bool {
    #[derive(serde::Deserialize)]
    struct Instruction {
        author: String,
        recipient: String,
        #[serde(default)]
        other_recipients: Vec<String>,
        #[serde(rename = "content")]
        _content: String,
        #[serde(rename = "trigger_turn")]
        _trigger_turn: bool,
        #[serde(rename = "id")]
        _id: Option<String>,
        #[serde(rename = "encrypted_content")]
        _encrypted_content: Option<String>,
        #[serde(rename = "internal_chat_message_metadata_passthrough")]
        _metadata: Option<InstructionMetadata>,
    }
    #[derive(serde::Deserialize)]
    struct InstructionMetadata {
        #[serde(rename = "turn_id")]
        _turn_id: Option<String>,
        #[serde(rename = "create_time")]
        _create_time: Option<serde_json::Number>,
    }
    fn valid_agent_path(path: &str) -> bool {
        if matches!(path, "/root" | "/morpheus") {
            return true;
        }
        path.strip_prefix("/root/").is_some_and(|suffix| {
            suffix.split('/').all(|segment| {
                !segment.is_empty()
                    && segment != "root"
                    && segment
                        .chars()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
            })
        })
    }
    let [ContentItem::InputText { text } | ContentItem::OutputText { text, .. }] = content else {
        return false;
    };
    serde_json::from_str::<Instruction>(text).is_ok_and(|instruction| {
        valid_agent_path(&instruction.author)
            && valid_agent_path(&instruction.recipient)
            && instruction
                .other_recipients
                .iter()
                .all(|path| valid_agent_path(path))
    })
}

#[must_use]
pub fn is_contextual_user_message(item: &ResponseItem) -> bool {
    let ResponseItem::Message { content, .. } = item else {
        return false;
    };
    content
        .iter()
        .filter_map(|content| {
            let ContentItem::InputText { text } = content else {
                return None;
            };
            Some(text.as_ref())
        })
        .any(|text| {
            matches_marked_text("# AGENTS.md instructions", "</INSTRUCTIONS>", text)
                || matches_marked_text("<environment_context>", "</environment_context>", text)
                || matches_marked_text("<turn_aborted>", "</turn_aborted>", text)
        })
}

pub(crate) fn is_canonical_context_item(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message {
            role: MessageRole::Developer,
            ..
        } => true,
        ResponseItem::Message {
            role: MessageRole::User,
            content,
            ..
        } => content.iter().any(|content| {
            let ContentItem::InputText { text } = content else {
                return false;
            };
            matches_marked_text("# AGENTS.md instructions", "</INSTRUCTIONS>", text)
                || matches_marked_text("<environment_context>", "</environment_context>", text)
        }),
        _ => false,
    }
}

fn matches_marked_text(start: &str, end: &str, text: &str) -> bool {
    let text = text.trim();
    text.get(..start.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(start))
        && text
            .get(text.len().saturating_sub(end.len())..)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(end))
}

const fn is_api_item(item: &ResponseItem) -> bool {
    !matches!(
        item,
        ResponseItem::CompactionTrigger {} | ResponseItem::Other(_)
    )
}

fn truncate_tool_output(mut item: ResponseItem) -> ResponseItem {
    let (ResponseItem::FunctionCallOutput { output, .. }
    | ResponseItem::CustomToolCallOutput { output, .. }) = &mut item
    else {
        return item;
    };
    match output {
        FunctionOutputBody::Text(text) => {
            *text = compaction::truncate_middle_with_token_budget(text, TOOL_OUTPUT_TOKEN_LIMIT)
                .into_boxed_str();
        }
        FunctionOutputBody::Content(content) => {
            truncate_output_content(content, TOOL_OUTPUT_TOKEN_LIMIT);
        }
    }
    item
}

fn truncate_output_content(items: &mut Vec<FunctionOutputContent>, token_limit: usize) {
    let mut remaining = token_limit;
    let mut omitted_text_items = 0usize;
    let mut output = Vec::with_capacity(items.len());
    for mut item in std::mem::take(items) {
        match &mut item {
            FunctionOutputContent::InputText { text } => {
                if remaining == 0 {
                    omitted_text_items += 1;
                    continue;
                }
                let tokens = text.len().div_ceil(4);
                if tokens <= remaining {
                    remaining -= tokens;
                    output.push(item);
                } else {
                    *text = compaction::truncate_middle_with_token_budget(text, remaining)
                        .into_boxed_str();
                    if text.is_empty() {
                        omitted_text_items += 1;
                    } else {
                        output.push(item);
                    }
                    remaining = 0;
                }
            }
            FunctionOutputContent::InputImage { .. }
            | FunctionOutputContent::InputImageFile { .. }
            | FunctionOutputContent::EncryptedContent { .. } => output.push(item),
            FunctionOutputContent::InputAudio { .. } => {}
        }
    }
    if omitted_text_items > 0 {
        output.push(FunctionOutputContent::InputText {
            text: format!("[omitted {omitted_text_items} text items ...]").into_boxed_str(),
        });
    }
    *items = output;
}
