use nanocode_agent_core::{NanocodeError, Result, SessionInfo, SteerReceiptStatus};
use serde_json::{Value, value::RawValue};
use std::{future::Future, pin::Pin, sync::Arc};

#[cfg(not(target_family = "wasm"))]
pub type PolicyFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

#[cfg(target_family = "wasm")]
pub type PolicyFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + 'a>>;

pub enum Admission {
    Execute,

    Resume,

    Completed { checkpoint: Value, output: Value },

    Failed { checkpoint: Value, error: String },

    Cancelled,
}

pub enum Step {
    Execute,

    Interrupted,

    Replay(Value),
}

#[doc(hidden)]
pub struct EncodedState {
    pub json: Box<RawValue>,

    pub records: Vec<nanocode_durability::EncodedPayload>,
}

#[doc(hidden)]
pub enum JournalWrite {
    Advance(EncodedState),

    Complete {
        step_id: String,
        output: Box<RawValue>,
        records: Vec<nanocode_durability::EncodedPayload>,
    },
}

#[doc(hidden)]
pub enum Settlement {
    Complete(Box<RawValue>),

    Fail(String),

    FailAttempt(String),

    Cancel,
}

#[doc(hidden)]
pub struct BeginRequest<'a> {
    pub step_id: String,

    pub kind: String,

    pub input: Box<RawValue>,

    pub wire: Option<&'a crate::PreparedWire>,
}

pub struct ClaudeSteer {
    pub message_id: Option<String>,

    pub index: u32,

    pub accepted_after_model_call_index: u32,

    pub model_call_index: Option<u32>,

    pub input_json: String,
}

pub trait ClaudeExecutionPolicy: Send + Sync {
    fn state_id(&self) -> &str;
    fn admit(
        &self,
        id: String,
        input: Value,
        automatic: bool,
    ) -> PolicyFuture<'_, (String, Admission)>;
    fn begin_attempt(&self, id: String) -> PolicyFuture<'_, ()>;

    fn supports_steering(&self) -> bool {
        false
    }

    fn identified_steer_accepted(
        &self,
        _message_id: String,
        _input_json: String,
    ) -> PolicyFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }

    fn steer_receipt(
        &self,
        _message_id: String,
        _input_json: String,
    ) -> PolicyFuture<'_, SteerReceiptStatus> {
        Box::pin(async { Ok(SteerReceiptStatus::Unknown) })
    }

    fn accept_steer(
        &self,
        _id: String,
        _message_id: Option<String>,
        _after: u32,
        _identity_json: String,
        _input_json: String,
    ) -> PolicyFuture<'_, Option<u32>> {
        Box::pin(async {
            Err(NanocodeError::InvalidRequest(
                "Claude execution policy does not support accept_steer".into(),
            ))
        })
    }
    fn retained_steers(&self, _id: String) -> PolicyFuture<'_, Vec<ClaudeSteer>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn withdraw_steer(&self, _id: String, _index: u32) -> PolicyFuture<'_, ()> {
        Box::pin(async {
            Err(NanocodeError::InvalidRequest(
                "Claude execution policy does not support withdraw_steer".into(),
            ))
        })
    }
    fn bind_steer(&self, _id: String, _index: u32, _boundary: u32) -> PolicyFuture<'_, ()> {
        Box::pin(async {
            Err(NanocodeError::InvalidRequest(
                "Claude execution policy does not support bind_steer".into(),
            ))
        })
    }
    fn continuation(&self, id: String) -> PolicyFuture<'_, Option<Value>>;
    fn advance(&self, id: String, state: Value) -> PolicyFuture<'_, ()>;

    #[allow(clippy::boxed_local)]
    fn advance_encoded(&self, id: String, state: Box<RawValue>) -> PolicyFuture<'_, ()> {
        match serde_json::from_str(state.get()) {
            Ok(state) => self.advance(id, state),
            Err(error) => Box::pin(async move {
                Err(NanocodeError::InvalidRequest(format!(
                    "invalid encoded Claude execution state: {error}"
                )))
            }),
        }
    }
    fn begin_step(
        &self,
        id: String,
        step_id: String,
        kind: String,
        input: Value,
    ) -> PolicyFuture<'_, Step>;

    #[allow(clippy::boxed_local)]
    fn begin_step_encoded(
        &self,
        id: String,
        step_id: String,
        kind: String,
        input: Box<RawValue>,
    ) -> PolicyFuture<'_, Step> {
        match serde_json::from_str(input.get()) {
            Ok(input) => self.begin_step(id, step_id, kind, input),
            Err(error) => Box::pin(async move {
                Err(NanocodeError::InvalidRequest(format!(
                    "invalid encoded Claude execution state: {error}"
                )))
            }),
        }
    }
    #[doc(hidden)]
    fn commit_writes<'a>(
        &'a self,
        id: String,
        writes: Vec<JournalWrite>,
        begin: Option<BeginRequest<'a>>,
    ) -> PolicyFuture<'a, Option<Step>> {
        Box::pin(async move {
            for write in writes {
                match write {
                    JournalWrite::Advance(state) => {
                        self.advance_encoded(id.clone(), state.json).await?;
                    }
                    JournalWrite::Complete {
                        step_id, output, ..
                    } => {
                        let output = serde_json::from_str(output.get())
                            .map_err(crate::agent::provider_error)?;
                        self.complete_step(id.clone(), step_id, output).await?;
                    }
                }
            }
            let Some(begin) = begin else {
                return Ok(None);
            };
            let input = match begin.wire {
                Some(wire) => wire
                    .legacy_durable_input()
                    .map_err(crate::agent::provider_error)?,
                None => begin.input,
            };
            self.begin_step_encoded(id, begin.step_id, begin.kind, input)
                .await
                .map(Some)
        })
    }
    #[doc(hidden)]
    fn defer_writes(&self, id: String, writes: Vec<JournalWrite>) -> PolicyFuture<'_, ()> {
        Box::pin(async move { self.commit_writes(id, writes, None).await.map(|_| ()) })
    }
    fn complete_step(&self, id: String, step_id: String, output: Value) -> PolicyFuture<'_, ()>;
    fn complete(&self, id: String, checkpoint: Value, output: Value) -> PolicyFuture<'_, ()>;
    fn fail(&self, id: String, checkpoint: Value, error: String) -> PolicyFuture<'_, ()>;
    fn fail_attempt(&self, id: String, checkpoint: Value, error: String) -> PolicyFuture<'_, ()> {
        self.fail(id, checkpoint, error)
    }
    fn cancel(&self, id: String, checkpoint: Value) -> PolicyFuture<'_, ()>;

    #[doc(hidden)]
    fn pages_messages(&self) -> bool {
        false
    }

    #[doc(hidden)]
    fn settle_encoded(
        &self,
        id: String,
        checkpoint: EncodedState,
        settlement: Settlement,
    ) -> PolicyFuture<'_, ()> {
        Box::pin(async move {
            let checkpoint: Value = serde_json::from_str(checkpoint.json.get())
                .map_err(crate::agent::provider_error)?;
            match settlement {
                Settlement::Complete(output) => {
                    let output =
                        serde_json::from_str(output.get()).map_err(crate::agent::provider_error)?;
                    self.complete(id, checkpoint, output).await
                }
                Settlement::Fail(error) => self.fail(id, checkpoint, error).await,
                Settlement::FailAttempt(error) => self.fail_attempt(id, checkpoint, error).await,
                Settlement::Cancel => self.cancel(id, checkpoint).await,
            }
        })
    }

    fn cancel_unstarted(&self, _id: String) -> PolicyFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }
    fn release(&self, id: String) -> PolicyFuture<'_, ()>;
    fn shutdown(&self) -> PolicyFuture<'_, ()>;
    fn checkpoint(&self, state: Value) -> PolicyFuture<'_, ()>;

    fn developer_notices(&self, _id: String) -> PolicyFuture<'_, Vec<(String, String)>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn initial_checkpoint(
        &self,
        _state: Value,
        _model: nanocode_agent_core::ModelSelection,
    ) -> PolicyFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn discard_initial(&self) -> PolicyFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn branch(&self, _child: &SessionInfo) -> Result<Option<Arc<dyn ClaudeExecutionPolicy>>> {
        Ok(None)
    }
}
