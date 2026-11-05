use super::{AgentEventData, AgentEventKind, AssistantEvent, ModelEvent};
use crate::responses::MessagePhase;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default)]
pub struct MessagePhases {
    stop_reasons: bool,
    completion: Option<(u32, Option<MessagePhase>)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Shown {
    pub phase: Option<MessagePhase>,
    pub complete: bool,
}

impl MessagePhases {
    #[must_use]
    pub const fn new(stop_reasons: bool) -> Self {
        Self {
            stop_reasons,
            completion: None,
        }
    }

    pub fn observe(&mut self, event: &AgentEventData) -> Option<Shown> {
        if let AgentEventData::Model(ModelEvent::CallCompleted(call)) = event {
            self.completion = Some((
                call.call_index,
                match call.status.as_str() {
                    "end_turn" if call.tool_calls == 0 => Some(MessagePhase::FinalAnswer),
                    "tool_use" | "pause_turn" => Some(MessagePhase::Commentary),
                    _ => None,
                },
            ));
        }
        let (index, phase, complete) = match event {
            AgentEventData::Assistant(AssistantEvent::Delta(delta)) => {
                (delta.model_call_index, delta.phase, false)
            }
            AgentEventData::Assistant(AssistantEvent::Message(message)) => {
                (message.model_call_index, message.phase, true)
            }
            _ => return None,
        };
        if phase.is_some() {
            return Some(Shown { phase, complete });
        }
        if !self.stop_reasons {
            return None;
        }
        let phase = self
            .completion
            .filter(|(call, _)| complete && *call == index)
            .and_then(|(_, phase)| phase);
        Some(Shown { phase, complete })
    }
}

#[derive(serde::Deserialize)]
struct Retrying {
    model_call_index: Option<u32>,
    error_class: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ModelRetries {
    retried: HashMap<u32, (&'static str, u32)>,
    exhausted: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Retry<'a> {
    Attempt {
        cause: &'static str,
    },
    Settled {
        model: &'a str,
        cause: &'static str,
        retries: u32,
        recovered: bool,
    },
}

impl ModelRetries {
    #[must_use]
    pub fn cause(class: &str) -> &'static str {
        match class {
            "premature_close"
            | "receive_transport"
            | "send_transport"
            | "handshake_transport"
            | "https_transport"
            | "transport"
            | "incomplete_stream" => "connection",
            "https_timeout" | "handshake_timeout" | "send_timeout" | "transport_timeout"
            | "stream_timeout" => "timeout",
            "https_rate_limit"
            | "api_rate_limit"
            | "handshake_rate_limit"
            | "http_rate_limit"
            | "stream_rate_limit" => "rate_limit",
            "api_overload" | "http_overloaded" | "stream_overloaded" => "overloaded",
            "https_server" | "api_server" | "handshake_server" | "api_failed"
            | "api_incomplete" | "http_server" | "stream_api_error" => "server",
            _ => "other",
        }
    }

    pub fn observe<'a>(&mut self, data: &'a AgentEventData) -> Option<Retry<'a>> {
        let (index, model, recovered) = match data {
            AgentEventData::Transport(event)
                if event.kind() == AgentEventKind::ModelAttemptRetrying =>
            {
                let Ok(Retrying {
                    model_call_index: Some(index),
                    error_class,
                }) = event.decode_payload::<Retrying>()
                else {
                    return None;
                };
                let cause = Self::cause(error_class.as_deref().unwrap_or_default());
                let retried = self.retried.entry(index).or_insert((cause, 0));
                *retried = (cause, retried.1 + 1);
                return Some(Retry::Attempt { cause });
            }
            AgentEventData::Model(ModelEvent::CallCompleted(call)) => {
                (call.call_index, call.model.as_str(), true)
            }
            AgentEventData::Model(ModelEvent::CallFailed(call)) => {
                (call.call_index, call.model.as_str(), false)
            }
            _ => return None,
        };
        let (cause, retries) = self.retried.remove(&index)?;
        if !recovered {
            self.exhausted = Some(cause);
        }
        Some(Retry::Settled {
            model,
            cause,
            retries,
            recovered,
        })
    }

    #[must_use]
    pub const fn exhausted(&self) -> Option<&'static str> {
        self.exhausted
    }
}
