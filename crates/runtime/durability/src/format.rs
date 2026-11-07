use std::{collections::HashSet, fmt, future::Future, pin::Pin, sync::Arc};

use nanocode_sync::Swap;

use nanocode_agent_core::{HarnessFamily, SessionCheckpoint, Thinking, session::TranscriptItem};
use serde_json::Value;

use crate::{EncodedPayload, Error, PayloadReader, Result, SessionRecord};

#[cfg(not(target_family = "wasm"))]
pub type FormatFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[cfg(target_family = "wasm")]
pub type FormatFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub struct AdmittedTurn<'a> {
    pub input: &'a Value,
    pub steers: usize,
}

pub trait CheckpointFormat: Send + Sync {
    fn family(&self) -> HarnessFamily;

    fn owns(&self, checkpoint: &Value) -> bool;

    fn hydrate<'a>(
        &'a self,
        _reader: PayloadReader<'a>,
        stored: Value,
    ) -> FormatFuture<'a, Result<Value>> {
        Box::pin(async move { Ok(stored) })
    }

    fn hydrate_noting<'a>(
        &'a self,
        reader: PayloadReader<'a>,
        stored: Value,
        _read: &'a mut HashSet<String>,
    ) -> FormatFuture<'a, Result<Value>> {
        self.hydrate(reader, stored)
    }

    fn hydrate_continuation<'a>(
        &'a self,
        _reader: PayloadReader<'a>,
        _continuation: Value,
    ) -> FormatFuture<'a, Result<Option<Value>>> {
        Box::pin(async { Ok(None) })
    }

    fn encode(&self, checkpoint: Value) -> Result<EncodedPayload> {
        EncodedPayload::encode(&checkpoint)
    }

    fn session_checkpoint(
        &self,
        record: &SessionRecord,
        checkpoint: Value,
    ) -> Result<SessionCheckpoint>;

    fn thinking(&self, record: &SessionRecord, checkpoint: &Value) -> Result<Thinking> {
        Ok(self
            .session_checkpoint(record, checkpoint.clone())?
            .thinking())
    }

    fn transcript(&self, checkpoint: &Value, turns: &[AdmittedTurn<'_>]) -> Vec<TranscriptItem>;
}

#[derive(Clone, Default)]
pub(crate) struct Formats(Arc<Swap<Vec<Arc<dyn CheckpointFormat>>>>);

impl Formats {
    pub(crate) fn register(&self, format: Arc<dyn CheckpointFormat>) -> Result<()> {
        let family = format.family();
        self.0.rcu(|formats| {
            formats
                .iter()
                .filter(|known| known.family() != family)
                .cloned()
                .chain(std::iter::once(Arc::clone(&format)))
                .collect::<Vec<_>>()
        });
        Ok(())
    }

    pub(crate) fn family(&self, family: HarnessFamily) -> Result<Arc<dyn CheckpointFormat>> {
        self.0
            .load()
            .iter()
            .find(|format| format.family() == family)
            .cloned()
            .ok_or_else(|| Error::UnsupportedFamily(family.to_string()))
    }

    pub(crate) fn owner(&self, checkpoint: &Value) -> Result<Arc<dyn CheckpointFormat>> {
        self.0
            .load()
            .iter()
            .find(|format| format.owns(checkpoint))
            .cloned()
            .ok_or_else(|| {
                Error::InvalidState("no registered checkpoint format owns this checkpoint".into())
            })
    }
}

impl fmt::Debug for Formats {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let families: Vec<_> = self.0.load().iter().map(|format| format.family()).collect();
        formatter.debug_list().entries(families).finish()
    }
}
