use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};

use crate::client::{ResponseItem, ResponsesError, Usage, responses::ResponseHistory};

use super::{
    compaction,
    context::{ContextManager, assign_missing_response_item_ids, has_well_formed_tool_calls},
};

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(try_from = "uuid::Uuid", into = "uuid::Uuid")]
pub struct SessionId(uuid::Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(nanocode_platform::uuid_v7())
    }

    #[must_use]
    pub const fn as_uuid(self) -> uuid::Uuid {
        self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SessionId").field(&self.0).finish()
    }
}

impl std::str::FromStr for SessionId {
    type Err = SessionIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(uuid::Uuid::parse_str(value)?)
    }
}

impl TryFrom<uuid::Uuid> for SessionId {
    type Error = SessionIdError;

    fn try_from(value: uuid::Uuid) -> Result<Self, Self::Error> {
        if value.get_version_num() != 7 {
            return Err(SessionIdError::WrongVersion {
                version: value.get_version_num(),
            });
        }
        Ok(Self(value))
    }
}

impl From<SessionId> for uuid::Uuid {
    fn from(value: SessionId) -> Self {
        value.0
    }
}

#[derive(Debug)]
pub enum SessionIdError {
    InvalidUuid(uuid::Error),

    WrongVersion { version: usize },
}

impl std::fmt::Display for SessionIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUuid(..) => formatter.write_str("invalid session UUID"),
            Self::WrongVersion { version } => {
                write!(formatter, "session IDs must be UUIDv7, got UUIDv{version}")
            }
        }
    }
}

impl std::error::Error for SessionIdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidUuid(value) => Some(value),
            _ => None,
        }
    }
}

impl From<uuid::Error> for SessionIdError {
    fn from(source: uuid::Error) -> Self {
        Self::InvalidUuid(source)
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub struct ManagedSessionState {
    context: ContextManager,
    client_authored: BTreeSet<String>,
    delta_start: usize,
    previous_response_id: Option<String>,
    history_revision: u64,
    server_reasoning_included: bool,
}

impl ManagedSessionState {
    #[must_use]
    pub fn new(mut items: Vec<ResponseItem>) -> Self {
        assign_missing_response_item_ids(&mut items);
        Self {
            context: ContextManager::new(items),
            client_authored: BTreeSet::new(),
            delta_start: 0,
            previous_response_id: None,
            history_revision: 0,
            server_reasoning_included: false,
        }
    }

    pub fn resume(mut items: Vec<ResponseItem>) -> Result<Self, ManagedSessionStateError> {
        if items.is_empty() {
            return Err(ManagedSessionStateError::EmptyHistory);
        }
        assign_missing_response_item_ids(&mut items);
        if !has_well_formed_tool_calls(&items) {
            return Err(ManagedSessionStateError::MalformedToolCalls);
        }
        let history_len = items.len();
        let mut state = Self::new(items);
        if state.context.len() != history_len {
            return Err(ManagedSessionStateError::UnsupportedHistoryItem);
        }
        if state.context.replace_invalid_tool_images() > 0 {
            state.history_revision = state.history_revision.saturating_add(1);
        }
        state.context.commit_tail();
        state.delta_start = state.context.len();
        Ok(state)
    }

    #[must_use]
    pub fn history_len(&self) -> usize {
        self.context.len()
    }

    #[must_use]
    pub fn history_is_empty(&self) -> bool {
        self.context.is_empty()
    }

    pub fn history(&self) -> impl ExactSizeIterator<Item = &ResponseItem> {
        self.context.iter()
    }

    #[must_use]
    pub fn flattened_history(&self) -> Vec<ResponseItem> {
        self.context.flattened_items()
    }

    #[must_use]
    pub fn shared_history(&self) -> ResponseHistory {
        self.context.shared_items()
    }

    pub fn append(&mut self, items: impl IntoIterator<Item = ResponseItem>) {
        self.context.record_items(items);
    }

    pub fn append_client(&mut self, items: impl IntoIterator<Item = ResponseItem>) {
        let mut items: Vec<_> = items.into_iter().collect();
        assign_missing_response_item_ids(&mut items);
        self.client_authored.extend(items.iter().filter_map(|item| {
            matches!(
                item,
                ResponseItem::Message {
                    role: crate::client::MessageRole::Developer,
                    ..
                }
            )
            .then(|| item.id().map(ToString::to_string))
            .flatten()
        }));
        self.append(items);
    }

    #[must_use]
    pub const fn client_authored(&self) -> &BTreeSet<String> {
        &self.client_authored
    }

    pub fn restore_client_authored(&mut self, ids: BTreeSet<String>) {
        self.client_authored = self
            .context
            .iter()
            .filter_map(|item| {
                let id = item.id()?;
                (matches!(
                    item,
                    ResponseItem::Message {
                        role: crate::client::MessageRole::Developer,
                        ..
                    }
                ) && ids.contains(id.as_str()))
                .then(|| id.to_string())
            })
            .collect();
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn context_usage(&self) -> (Option<&Usage>, bool) {
        (
            self.context.last_token_usage.as_ref(),
            self.server_reasoning_included,
        )
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn context_usage_is_estimate(&self) -> bool {
        self.context.token_usage_is_estimate
    }

    #[doc(hidden)]
    pub fn restore_context_usage(
        &mut self,
        usage: Option<&Usage>,
        server_reasoning_included: bool,
        is_estimate: bool,
    ) {
        self.observe_server_reasoning(server_reasoning_included);
        if self.history_revision == 0 {
            self.context.update_token_info(usage);
            self.context.token_usage_is_estimate = usage.is_some() && is_estimate;
        }
    }

    pub fn update_token_info(&mut self, usage: Option<&Usage>) {
        self.context.update_token_info(usage);
    }

    pub const fn observe_server_reasoning(&mut self, included: bool) {
        self.server_reasoning_included |= included;
    }

    #[must_use]
    pub fn active_context_tokens(&self) -> u64 {
        self.context
            .active_context_tokens(self.server_reasoning_included)
    }

    #[must_use]
    pub const fn delta_start(&self) -> usize {
        self.delta_start
    }

    #[must_use]
    pub fn previous_response_id(&self) -> Option<&str> {
        self.previous_response_id.as_deref()
    }

    pub fn set_previous_response_id(&mut self, response_id: impl Into<String>) {
        let response_id = response_id.into();
        self.previous_response_id = (!response_id.is_empty()).then_some(response_id);
    }

    pub fn clear_delta(&mut self) {
        self.delta_start = self.context.len();
    }

    pub fn reset_for_full_request(&mut self) {
        self.delta_start = 0;
        self.previous_response_id = None;
    }

    pub fn commit(&mut self) -> Result<(), ManagedSessionStateError> {
        if self.previous_response_id.is_none() {
            return Err(ManagedSessionStateError::MissingResponseId);
        }
        self.context.commit_tail();
        self.delta_start = self.context.len();
        Ok(())
    }

    pub fn commit_interrupted(&mut self) {
        self.reset_for_full_request();
        self.context.commit_tail();
    }

    #[must_use]
    pub fn generation_request(&self) -> RequestHistory {
        let (full, repaired) = self.context.prompt_items_with_repair();
        RequestHistory {
            full,
            incremental: self.context.shared_items(),
            incremental_start: self.delta_start,
            previous_response_id: self.previous_response_id.clone().filter(|_| !repaired),
            repaired,
        }
    }

    pub fn complete_generation(
        &mut self,
        request: RequestHistory,
        response_id: Option<String>,
        output: impl IntoIterator<Item = ResponseItem>,
        usage: Option<&Usage>,
        server_reasoning_included: bool,
    ) -> Result<(), ManagedSessionStateError> {
        if response_id.as_deref().is_some_and(str::is_empty) {
            return Err(ManagedSessionStateError::MissingResponseId);
        }
        self.observe_server_reasoning(server_reasoning_included);
        if request.repaired {
            self.context.adopt_prompt_items(request.full);
        }
        self.update_token_info(usage);
        match response_id {
            Some(response_id) => self.previous_response_id = Some(response_id),
            None => self.reset_for_full_request(),
        }
        self.append(output);
        Ok(())
    }

    #[must_use]
    pub fn compaction_request(&self) -> RequestHistory {
        let (full, repaired) = self.context.prompt_items_with_repair();
        RequestHistory {
            incremental: full.clone(),
            full,
            incremental_start: if repaired { 0 } else { self.delta_start },
            previous_response_id: self.previous_response_id.clone().filter(|_| !repaired),
            repaired,
        }
    }

    pub fn repair_rejected_request(&mut self, repair: RejectedRequestRepair<'_>) -> bool {
        let mut changed = 0;
        if repair.replace_images {
            changed += self.context.replace_rejected_images();
        }
        if let Some(definition) = repair.tool_definition {
            changed += self.context.remove_tool_definition(definition);
        }
        if changed > 0 {
            self.reset_for_full_request();
            self.history_revision = self.history_revision.saturating_add(1);
        }
        changed > 0
    }

    pub fn commit_tail(&mut self) {
        self.context.commit_tail();
    }

    pub fn install_compaction(
        &mut self,
        item: ResponseItem,
        initial_context: impl IntoIterator<Item = ResponseItem>,
        request_prefix: &[ResponseItem],
    ) {
        let initial_context = initial_context.into_iter().collect::<Vec<_>>();
        let history = compaction::install_history_with_provenance(
            &self.context.flattened_items(),
            &initial_context,
            item,
            &self.client_authored,
        );
        self.context.replace_and_recompute(history, request_prefix);
        let provenance = std::mem::take(&mut self.client_authored);
        self.restore_client_authored(provenance);
        self.reset_for_full_request();
        self.history_revision = self.history_revision.saturating_add(1);
    }

    #[doc(hidden)]
    pub fn replace_prepared_history(&mut self, history: Vec<ResponseItem>) {
        self.context.replace_and_recompute(history, &[]);
        self.context.commit_tail();
        self.reset_for_full_request();
        self.history_revision = self.history_revision.saturating_add(1);
    }

    #[must_use]
    pub const fn history_revision(&self) -> u64 {
        self.history_revision
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub struct RequestHistory {
    pub(crate) full: ResponseHistory,
    pub(crate) incremental: ResponseHistory,
    pub(crate) incremental_start: usize,
    pub(crate) previous_response_id: Option<String>,
    pub(crate) repaired: bool,
}

impl RequestHistory {
    #[must_use]
    pub const fn full(&self) -> &ResponseHistory {
        &self.full
    }

    #[must_use]
    pub fn previous_response_id(&self) -> Option<&str> {
        self.previous_response_id.as_deref()
    }

    #[must_use]
    pub const fn incremental_start(&self) -> usize {
        self.incremental_start
    }

    pub fn push(&mut self, item: ResponseItem) {
        self.full.push(item.clone());
        self.incremental.push(item);
    }

    pub fn replace_from(&mut self, start: usize, items: Vec<ResponseItem>, continued: bool) {
        self.full.replace_suffix(start, items);
        self.incremental = self.full.clone();
        if !continued {
            self.incremental_start = 0;
            self.previous_response_id = None;
        }
    }

    pub fn fit_context_window(
        &mut self,
        request_prefix: &[ResponseItem],
        context_window_tokens: u64,
    ) {
        let rewritten = compaction::trim_tool_outputs_to_fit_context_window(
            &mut self.full,
            request_prefix,
            context_window_tokens,
        );
        if rewritten > 0 {
            self.incremental = self.full.clone();
            self.incremental_start = 0;
            self.previous_response_id = None;
        }
    }
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RejectedRequestRepair<'a> {
    pub replace_images: bool,

    pub tool_definition: Option<&'a serde_json::Value>,
}

impl<'a> RejectedRequestRepair<'a> {
    #[must_use]
    pub fn for_error(error: &'a ResponsesError) -> Self {
        Self {
            replace_images: matches!(error, ResponsesError::InvalidImageRequest { .. }),
            tool_definition: error.invalid_tool_schema(),
        }
    }
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedSessionStateError {
    EmptyHistory,

    UnsupportedHistoryItem,

    MalformedToolCalls,

    MissingResponseId,
}

impl std::fmt::Display for ManagedSessionStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyHistory => formatter.write_str("conversation history must not be empty"),
            Self::UnsupportedHistoryItem => {
                formatter.write_str("conversation history contains an unsupported item")
            }
            Self::MalformedToolCalls => formatter
                .write_str("conversation history contains an unmatched or misordered tool call"),
            Self::MissingResponseId => {
                formatter.write_str("completed response did not have a response ID")
            }
        }
    }
}

impl std::error::Error for ManagedSessionStateError {}
