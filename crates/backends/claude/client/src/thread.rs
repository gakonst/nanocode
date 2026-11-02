//! Message threads (beta): the Messages API holds a conversation between requests, so each
//! request after the first sends only the messages that follow the previous response.
//!
//! A thread is best-effort. The API keeps it for [`THREAD_IDLE_EXPIRY`] after the last response
//! it stored, may decline to store a response, and answers a continue request it cannot serve
//! with an error before the model runs. The caller therefore keeps the whole conversation and
//! must be able to send it again at any time.
//!
//! [`MessageThread`] makes that decision for one conversation without doing any I/O:
//!
//! 1. [`MessageThread::plan`] reads the request and the full conversation and returns a
//!    [`ThreadTurn`]: create a thread, continue the stored one, or send without a thread.
//! 2. [`ClaudeClient::attach_thread`](crate::ClaudeClient::attach_thread) makes a prepared
//!    request send that turn.
//! 3. [`MessageThread::complete`] records the response, or [`MessageThread::recover`] reads the
//!    error and says whether to send the request again at once.
//!
//! Cache each turn with the request's top-level `cache_control`, which is a fixed field and
//! applies to the newest messages of every request. Breakpoints placed on message blocks are
//! sent as given, on a create that replays the conversation too, so they count against the
//! limit of four on every request.
//!
//! See <https://platform.claude.com/docs/en/build-with-claude/message-threads>.

use std::time::Duration;

use nanocode_platform::Instant;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CacheControl, ClaudeError, ClaudeToolSpec, ContentBlock, Message, MessageResponse,
    MessagesRequest, OutputConfig, Role, Speed, StopReason, WireMessage,
};

/// The `anthropic-beta` value that every request of a thread sends.
pub const MESSAGE_THREADS_BETA: &str = "message-threads-2026-08-12";

/// How long the API keeps a thread after the last response it stored, as documented.
pub const THREAD_IDLE_EXPIRY: Duration = Duration::from_secs(5 * 60);

/// The request's `thread` field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThreadParam {
    /// Hold this conversation under the response's `id`.
    Create,
    /// Resume from the response named by `previous_message_id`.
    Continue { previous_message_id: String },
}

/// The `error.details.error_code` of an error that a thread request caused.
///
/// The API returns each of these before the model runs: the request used no tokens and left
/// the thread and its expiry unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadErrorCode {
    /// `thread_not_found`: the API can no longer continue the thread.
    NotFound,
    /// `thread_fingerprint_mismatch`: a fixed field differs from the thread's.
    FingerprintMismatch,
    /// `thread_already_continued`: the response was already continued.
    AlreadyContinued,
    /// `thread_continue_in_flight`: another continue from the response is still running.
    ContinueInFlight,
    /// `thread_unsupported_request`: the request or the organization cannot use threads.
    UnsupportedRequest,
}

impl ThreadErrorCode {
    /// Parses an `error_code` value.
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
    /// The thread error code this HTTP error carries, if any.
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

/// What one request of a conversation sends, from [`MessageThread::plan`].
#[derive(Clone, Debug)]
pub struct ThreadTurn {
    param: Option<ThreadParam>,
    /// The messages the thread already holds, which a continue leaves out.
    held: usize,
    /// The number of messages in the conversation the turn was planned for.
    sent: usize,
    /// Every message of that conversation, hashed in order.
    digest: blake3::Hasher,
    fixed: blake3::Hash,
    /// This create sends again a conversation whose thread the API lost.
    recreated: bool,
}

impl ThreadTurn {
    /// The request's `thread` field, or `None` to send without a thread.
    #[must_use]
    pub const fn param(&self) -> Option<&ThreadParam> {
        self.param.as_ref()
    }

    /// The messages this turn sends: on a continue, those after the previous response.
    ///
    /// `messages` must be the conversation the turn was planned for: the turn recorded its
    /// digest, and [`MessageThread::complete`] assumes the API now holds exactly it.
    ///
    /// # Panics
    ///
    /// When `messages` is not as long as that conversation.
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

/// What to do after a request of a thread failed, from [`MessageThread::recover`].
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadRecovery {
    /// The thread caused the error and the API ran nothing: plan the request again and send it
    /// at once. It creates a new thread or goes without one.
    Resend,
    /// The error is not the thread's. Handle it as any other; a retry may continue the thread.
    Unrelated,
}

/// The thread state of one conversation.
///
/// Planning compares the conversation with what the thread holds, so the caller may edit,
/// compact or replace its history at any time: a conversation that no longer extends the stored
/// one starts a new thread.
#[derive(Clone, Debug, Default)]
pub struct MessageThread {
    state: State,
}

#[derive(Clone, Debug)]
enum State {
    /// Nothing to continue: the next request creates a thread. `recreating` when the API
    /// lost the previous thread, so that losing the new one too ends threading.
    Fresh {
        recreating: bool,
    },
    Stored(Anchor),
    /// The API cannot hold this conversation: send it whole.
    Unthreaded,
}

impl Default for State {
    fn default() -> Self {
        Self::Fresh { recreating: false }
    }
}

#[derive(Clone, Debug)]
struct Anchor {
    response_id: String,
    /// The number of messages the thread holds, the response last.
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

    /// Whether the API refused threads for this conversation, which then goes without one.
    #[must_use]
    pub const fn is_unthreaded(&self) -> bool {
        matches!(self.state, State::Unthreaded)
    }

    /// Plans the request `head` with the full conversation `messages` at `now`.
    ///
    /// It continues the stored thread only when `messages` begins with exactly the messages the
    /// thread holds, `head` keeps the thread's create-only and fixed fields, and the thread has
    /// not expired. Otherwise it creates a new thread.
    pub fn plan<M: WireMessage>(
        &self,
        head: &MessagesRequest,
        messages: &[M],
        now: Instant,
    ) -> Result<ThreadTurn, ClaudeError> {
        let fixed = fingerprint(head)?;
        let mut digest = blake3::Hasher::new();
        let mut hashed = 0;
        let (param, held, recreated) = match &self.state {
            State::Unthreaded => (None, 0, false),
            State::Fresh { recreating } => (Some(ThreadParam::Create), 0, *recreating),
            State::Stored(anchor)
                if anchor.fixed == fixed
                    && anchor.messages <= messages.len()
                    && now.saturating_duration_since(anchor.stored_at) < THREAD_IDLE_EXPIRY =>
            {
                for message in &messages[..anchor.messages] {
                    absorb(&mut digest, message)?;
                }
                hashed = anchor.messages;
                if digest.finalize() == anchor.digest {
                    let previous_message_id = anchor.response_id.clone();
                    (
                        Some(ThreadParam::Continue {
                            previous_message_id,
                        }),
                        anchor.messages,
                        anchor.recreated,
                    )
                } else {
                    (Some(ThreadParam::Create), 0, false)
                }
            }
            State::Stored(_) => (Some(ThreadParam::Create), 0, false),
        };
        if param.is_some() {
            for message in &messages[hashed..] {
                absorb(&mut digest, message)?;
            }
        }
        Ok(ThreadTurn {
            param,
            held,
            sent: messages.len(),
            digest,
            fixed,
            recreated,
        })
    }

    /// Records the response to `turn`, which completed at `completed_at`.
    ///
    /// The API restarts a thread's expiry when it stores the response, so measure
    /// `completed_at` when the response ended. A continue sent just before the API's expiry may
    /// still find the thread gone, which [`MessageThread::recover`] handles.
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
            self.state = State::default();
            return Ok(());
        }
        // The reply as the caller appends it to its conversation, hashed as `plan` hashes the
        // conversation, so the two cannot encode it differently.
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
            recreated: matches!(turn.param, Some(ThreadParam::Create)) && turn.recreated,
        });
        Ok(())
    }

    /// Reads the error that `turn` failed with.
    ///
    /// A thread error moves to a new thread, or to none when the API cannot hold this
    /// conversation, and returns [`ThreadRecovery::Resend`]. That happens at most twice in a
    /// row: a continue the API refuses becomes a create, and only a create the API refuses
    /// becomes unthreaded. Any other error leaves the state as it was, so a retry continues the
    /// same response; should the failed attempt have consumed it, the retry fails with a thread
    /// error and is sent again as a create.
    pub fn recover(&mut self, turn: &ThreadTurn, error: &ClaudeError) -> ThreadRecovery {
        let Some(param) = &turn.param else {
            return ThreadRecovery::Unrelated;
        };
        let continuing = matches!(param, ThreadParam::Continue { .. });
        self.state = match error.thread_error() {
            Some(ThreadErrorCode::UnsupportedRequest) => State::Unthreaded,
            // Losing a thread right after sending it again means the API cannot store it.
            Some(ThreadErrorCode::NotFound) if continuing && turn.recreated => State::Unthreaded,
            Some(ThreadErrorCode::NotFound) if continuing => State::Fresh { recreating: true },
            Some(
                ThreadErrorCode::AlreadyContinued
                | ThreadErrorCode::ContinueInFlight
                | ThreadErrorCode::FingerprintMismatch,
            ) if continuing => State::default(),
            _ => return ThreadRecovery::Unrelated,
        };
        ThreadRecovery::Resend
    }
}

/// Whether the API keeps a response in its thread, so that a request can continue from it.
///
/// A refused turn must not be continued, and the API stores no response that ends in a tool
/// call cut off by `max_tokens`.
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

/// Hashes the create-only and fixed fields, which a thread keeps from its create request.
///
/// `speed` is a free field, but it selects an `anthropic-beta` value, and the set of those is
/// fixed.
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
        system: head.system.as_ref(),
        tools: &head.tools,
    })?;
    Ok(blake3::hash(&fixed))
}

fn absorb<M: WireMessage>(digest: &mut blake3::Hasher, message: &M) -> Result<(), ClaudeError> {
    let encode = |message: &Message| serde_json::to_string(message).map_err(ClaudeError::from);
    absorb_encoded(digest, &message.encoded(false, &encode)?);
    Ok(())
}

/// Hashes one encoded message, length first so that message boundaries count.
fn absorb_encoded(digest: &mut blake3::Hasher, encoded: &str) {
    digest.update(&(encoded.len() as u64).to_le_bytes());
    digest.update(encoded.as_bytes());
}
