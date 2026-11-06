use std::time::Duration;

use nanocode_platform::Instant;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CacheControl, ClaudeError, ClaudeToolSpec, ContentBlock, Message, MessageResponse,
    MessagesRequest, OutputConfig, Role, Speed, StopReason, WireMessage,
};

pub const MESSAGE_THREADS_BETA: &str = "message-threads-2026-08-12";

pub const THREAD_IDLE_EXPIRY: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThreadParam {
    Create,
    Continue { previous_message_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadErrorCode {
    NotFound,
    FingerprintMismatch,
    AlreadyContinued,
    ContinueInFlight,
    UnsupportedRequest,
}

impl ThreadErrorCode {
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "thread_not_found" => Self::NotFound,
            "thread_fingerprint_mismatch" => Self::FingerprintMismatch,
            "thread_already_continued" => Self::AlreadyContinued,
            "thread_continue_in_flight" => Self::ContinueInFlight,
            "thread_unsupported_request" => Self::UnsupportedRequest,
            _ => return None,
        })
    }
}

impl ClaudeError {
    #[must_use]
    pub fn thread_error(&self) -> Option<ThreadErrorCode> {
        #[derive(Deserialize)]
        struct Body<'a> {
            #[serde(borrow)]
            error: Error<'a>,
        }
        #[derive(Deserialize)]
        struct Error<'a> {
            #[serde(borrow)]
            details: Option<Details<'a>>,
        }
        #[derive(Deserialize)]
        struct Details<'a> {
            error_code: Option<&'a str>,
        }
        let Self::Http { body, .. } = self else {
            return None;
        };
        serde_json::from_str::<Body<'_>>(body)
            .ok()?
            .error
            .details?
            .error_code
            .and_then(ThreadErrorCode::from_code)
    }
}

#[derive(Clone, Debug)]
pub struct ThreadTurn {
    param: Option<ThreadParam>,
    mode: ThreadMode,
    held: usize,
    sent: usize,
    digest: blake3::Hasher,
    fixed: blake3::Hash,
    recreated: bool,
}

impl ThreadTurn {
    #[must_use]
    pub const fn param(&self) -> Option<&ThreadParam> {
        self.param.as_ref()
    }

    #[must_use]
    pub const fn mode(&self) -> ThreadMode {
        self.mode
    }

    #[must_use]
    pub fn new_messages<'a, M>(&self, messages: &'a [M]) -> &'a [M] {
        assert_eq!(
            messages.len(),
            self.sent,
            "a thread turn sends the conversation it was planned for"
        );
        &messages[self.held..]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadMode {
    Continue,
    Create(CreateReason),
    Unthreaded,
}

impl ThreadMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Create(_) => "create",
            Self::Unthreaded => "unthreaded",
        }
    }

    #[must_use]
    pub const fn create_reason(self) -> Option<CreateReason> {
        match self {
            Self::Create(reason) => Some(reason),
            Self::Continue | Self::Unthreaded => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateReason {
    First,
    NotStored,
    Lost,
    Consumed,
    Expired,
    FieldsChanged,
    HistoryDiverged,
}

impl CreateReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::NotStored => "not_stored",
            Self::Lost => "lost",
            Self::Consumed => "consumed",
            Self::Expired => "expired",
            Self::FieldsChanged => "fields_changed",
            Self::HistoryDiverged => "history_diverged",
        }
    }
}

#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadRecovery {
    Resend,
    Unrelated,
}

#[derive(Clone, Debug, Default)]
pub struct MessageThread {
    state: State,
}

#[derive(Clone, Debug)]
enum State {
    Fresh(CreateReason),
    Stored(Anchor),
    Unthreaded,
}

impl Default for State {
    fn default() -> Self {
        Self::Fresh(CreateReason::First)
    }
}

#[derive(Clone, Debug)]
struct Anchor {
    response_id: String,
    messages: usize,
    digest: blake3::Hash,
    fixed: blake3::Hash,
    stored_at: Instant,
    recreated: bool,
}

impl MessageThread {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn is_unthreaded(&self) -> bool {
        matches!(self.state, State::Unthreaded)
    }

    pub fn plan<M: WireMessage>(
        &self,
        head: &MessagesRequest,
        messages: &[M],
        now: Instant,
    ) -> Result<ThreadTurn, ClaudeError> {
        let fixed = fingerprint(head)?;
        let mut turn = ThreadTurn {
            param: None,
            mode: ThreadMode::Unthreaded,
            held: 0,
            sent: messages.len(),
            digest: blake3::Hasher::new(),
            fixed,
            recreated: false,
        };
        let continued = match &self.state {
            State::Unthreaded => return Ok(turn),
            State::Fresh(reason) => Err(*reason),
            State::Stored(anchor) if anchor.fixed != fixed => Err(CreateReason::FieldsChanged),
            State::Stored(anchor)
                if now.saturating_duration_since(anchor.stored_at) >= THREAD_IDLE_EXPIRY =>
            {
                Err(CreateReason::Expired)
            }
            State::Stored(anchor) if anchor.messages > messages.len() => {
                Err(CreateReason::HistoryDiverged)
            }
            State::Stored(anchor) => {
                for message in &messages[..anchor.messages] {
                    absorb(&mut turn.digest, message)?;
                }
                turn.held = anchor.messages;
                if turn.digest.finalize() == anchor.digest {
                    Ok(anchor)
                } else {
                    Err(CreateReason::HistoryDiverged)
                }
            }
        };
        for message in &messages[turn.held..] {
            absorb(&mut turn.digest, message)?;
        }
        match continued {
            Ok(anchor) => {
                turn.param = Some(ThreadParam::Continue {
                    previous_message_id: anchor.response_id.clone(),
                });
                turn.mode = ThreadMode::Continue;
                turn.recreated = anchor.recreated;
            }
            Err(reason) => {
                turn.param = Some(ThreadParam::Create);
                turn.mode = ThreadMode::Create(reason);
                turn.held = 0;
                turn.recreated = reason == CreateReason::Lost;
            }
        }
        Ok(turn)
    }

    pub fn complete(
        &mut self,
        turn: &ThreadTurn,
        response: &MessageResponse,
        completed_at: Instant,
    ) -> Result<(), ClaudeError> {
        if turn.param.is_none() {
            return Ok(());
        }
        if !stored(response) {
            self.state = State::Fresh(CreateReason::NotStored);
            return Ok(());
        }
        let reply = Message {
            role: Role::Assistant,
            content: response.content.clone(),
        };
        let mut digest = turn.digest.clone();
        absorb(&mut digest, &reply)?;
        self.state = State::Stored(Anchor {
            response_id: response.id.clone(),
            messages: turn.sent + 1,
            digest: digest.finalize(),
            fixed: turn.fixed,
            stored_at: completed_at,
            recreated: turn.mode == ThreadMode::Create(CreateReason::Lost),
        });
        Ok(())
    }

    pub fn recover(&mut self, turn: &ThreadTurn, error: &ClaudeError) -> ThreadRecovery {
        if turn.param.is_none() {
            return ThreadRecovery::Unrelated;
        }
        let continuing = turn.mode == ThreadMode::Continue;
        self.state = match error.thread_error() {
            Some(ThreadErrorCode::UnsupportedRequest) => State::Unthreaded,
            Some(ThreadErrorCode::NotFound) if continuing && turn.recreated => State::Unthreaded,
            Some(ThreadErrorCode::NotFound) if continuing => State::Fresh(CreateReason::Lost),
            Some(ThreadErrorCode::AlreadyContinued | ThreadErrorCode::ContinueInFlight)
                if continuing =>
            {
                State::Fresh(CreateReason::Consumed)
            }
            Some(ThreadErrorCode::FingerprintMismatch) if continuing => {
                State::Fresh(CreateReason::FieldsChanged)
            }
            _ => return ThreadRecovery::Unrelated,
        };
        ThreadRecovery::Resend
    }
}

fn stored(response: &MessageResponse) -> bool {
    match response.stop_reason {
        Some(StopReason::Refusal) => false,
        Some(StopReason::MaxTokens) => !matches!(
            response.content.last(),
            Some(ContentBlock::ToolUse { .. } | ContentBlock::ServerToolUse { .. })
        ),
        _ => true,
    }
}

fn fingerprint(head: &MessagesRequest) -> Result<blake3::Hash, ClaudeError> {
    #[derive(Serialize)]
    struct Fixed<'a> {
        model: &'a str,
        cache_control: Option<&'a CacheControl>,
        output_config: Option<&'a OutputConfig>,
        speed: Option<Speed>,
        tool_choice: Option<&'a Value>,
        thinking: Option<&'a Value>,
        context_management: Option<&'a Value>,
        system: Option<&'a Value>,
        tools: &'a [ClaudeToolSpec],
    }
    let fixed = serde_json::to_vec(&Fixed {
        model: &head.model,
        cache_control: head.cache_control.as_ref(),
        output_config: head.output_config.as_ref(),
        speed: head.speed,
        tool_choice: head.tool_choice.as_ref(),
        thinking: head.thinking.as_ref(),
        context_management: head.context_management.as_ref(),
        system: head.system.as_deref(),
        tools: &head.tools,
    })?;
    Ok(blake3::hash(&fixed))
}

fn absorb<M: WireMessage>(digest: &mut blake3::Hasher, message: &M) -> Result<(), ClaudeError> {
    let encode = |message: &Message| serde_json::to_string(message).map_err(ClaudeError::from);
    absorb_encoded(digest, &message.encoded(false, &encode)?);
    Ok(())
}

fn absorb_encoded(digest: &mut blake3::Hasher, encoded: &str) {
    digest.update(&(encoded.len() as u64).to_le_bytes());
    digest.update(encoded.as_bytes());
}
