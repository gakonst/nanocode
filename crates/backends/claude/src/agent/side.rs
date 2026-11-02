use super::{
    Cancellation, ContentBlock, HistoryMessage, Message, MessagesRequest, NanocodeError,
    ResponseContext, Result, Role, State, provider_error,
};
use crate::{FrozenWireProfile, ToolResultContent};
use nanocode_agent_core::{
    SideAnswer,
    backend::{TranscriptBuilder, side_question_prompt},
    input::PromptMessage,
};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

fn tool_result_text(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text(text) => text.clone(),
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .map(|block| match block.get("type").and_then(Value::as_str) {
                Some("text") => block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                Some("image") => "[image]".to_owned(),
                _ => block.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

pub(super) fn claude_transcript(messages: &[HistoryMessage]) -> Vec<PromptMessage> {
    let mut transcript = TranscriptBuilder::default();
    let mut tools = HashMap::new();
    for message in messages {
        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } => match message.role {
                    Role::User => transcript.user(text),
                    Role::Assistant => transcript.assistant(text),
                },
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    tools.insert(id.clone(), name.clone());
                    transcript.tool_call(name, &input.to_string());
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => transcript.tool_result(
                    tools
                        .get(tool_use_id)
                        .map_or(tool_use_id.as_str(), String::as_str),
                    &tool_result_text(content),
                ),
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
                ContentBlock::Image { .. } => match message.role {
                    Role::User => transcript.user("[image]"),
                    Role::Assistant => transcript.assistant("[image]"),
                },
                ContentBlock::Document { .. } => match message.role {
                    Role::User => transcript.user("[document]"),
                    Role::Assistant => transcript.assistant("[document]"),
                },
                other => {
                    let rendered = serde_json::to_string(other).unwrap_or_default();
                    match message.role {
                        Role::User => transcript.user(&rendered),
                        Role::Assistant => transcript.assistant(&rendered),
                    }
                }
            }
        }
    }
    transcript.finish()
}

pub(super) type SideRequest = (MessagesRequest, Option<FrozenWireProfile>);

const RUNNING_TOOL_RESULT: &str =
    "This tool call is still running in the main conversation; its result is not available yet.";

const PENDING_REQUEST: &str = "<system-reminder>The user message above is still being handled by the main agent. Do not answer it or act on it here; only answer the side question below.</system-reminder>";

fn side_messages(mut messages: Vec<HistoryMessage>, question: &str) -> Vec<HistoryMessage> {
    let running = messages
        .last()
        .filter(|message| message.role == Role::Assistant)
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, .. } => {
                        Some(ContentBlock::tool_result(id, RUNNING_TOOL_RESULT, false))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let text = ContentBlock::text(side_question_prompt(question));
    match messages.last_mut() {
        Some(message) if message.role == Role::User => {
            let message = message.make_mut();
            if !message
                .content
                .iter()
                .all(|block| matches!(block, ContentBlock::ToolResult { .. }))
            {
                message.content.push(ContentBlock::text(PENDING_REQUEST));
            }
            message.content.push(text);
        }
        _ => messages.push(
            Message {
                role: Role::User,
                content: running.into_iter().chain([text]).collect(),
            }
            .into(),
        ),
    }
    messages
}

impl State {
    pub(super) async fn transcript(&self) -> Result<Vec<PromptMessage>> {
        if self.life.stopped() {
            return Err(NanocodeError::AgentStopped);
        }
        let snapshot = self.latest_boundary().await?;
        Ok(claude_transcript(&snapshot.conversation.packed_messages()))
    }

    pub(super) fn publish_side_view(&self, messages: Option<Arc<[HistoryMessage]>>) {
        self.side_view.set(messages);
    }

    pub(super) fn publish_side_request(
        &self,
        template: &MessagesRequest,
        wire_profile: Option<&FrozenWireProfile>,
    ) {
        self.side_request
            .set(Some(Arc::new((template.clone(), wire_profile.cloned()))));
    }

    fn side_template(&self) -> SideRequest {
        let latest = Option::clone(&self.side_request.get());
        if let Some(latest) = latest {
            return SideRequest::clone(&latest);
        }
        let template = self.request_template(self.speed());
        let wire_profile = self
            .client
            .freeze_wire_profile(template.cache_control.is_some());
        (template, Some(wire_profile))
    }

    async fn side_transcript(&self) -> Result<Vec<HistoryMessage>> {
        let live = self
            .conversation
            .try_acquire()
            .is_none()
            .then(|| Option::clone(&self.side_view.get()));
        if let Some(Some(messages)) = live {
            return Ok(messages.to_vec());
        }
        Ok(self.latest_boundary().await?.conversation.packed_messages())
    }
}

impl State {
    pub(super) async fn side_question(&self, question: String) -> Result<SideAnswer> {
        if self.life.stopped() {
            return Err(NanocodeError::AgentStopped);
        }
        let transcript = self.side_transcript().await?;
        let (mut template, wire_profile) = self.side_template();
        let breakpoint = crate::cache_tail(&transcript);
        let mut messages = side_messages(transcript, &question);
        if let (Some(control), Some((message, block))) =
            (template.cache_control.clone(), breakpoint)
        {
            template
                .cache_system_prefix_with(&messages)
                .map_err(provider_error)?;
            template.cache_control = None;
            messages[message].make_mut().content[block]
                .set_cache_control(&control)
                .map_err(provider_error)?;
        }
        let response = self
            .response(
                &messages,
                template.tools.clone(),
                &Cancellation::default(),
                None,
                0,
                ResponseContext {
                    origin: self.host_context.clone(),
                    disable_tools: false,
                    force_text: false,
                    container: None,
                    previous_message_id: None,
                    template: Some(&template),
                    wire_profile: wire_profile.as_ref(),
                    effect: None,
                    journal: None,
                    threaded: false,
                },
            )
            .await
            .map_err(|failure| failure.error)?
            .message;
        let text = response
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        if text.trim().is_empty() {
            return Err(provider_error(
                if response
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
                {
                    "the side question response called a tool instead of answering directly"
                } else {
                    "the side question response contained no text"
                },
            ));
        }
        Ok(SideAnswer::new(
            text,
            Some(super::turn_usage(&response.usage)),
        ))
    }
}
