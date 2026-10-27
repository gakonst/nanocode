use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use nanocode_agent_core::TurnUsage;
use nanocode_durability::{
    EncodedPayload, Error, PagedWriter, PayloadReader, Prepared, Result, Sequence, SequenceLog,
    load_sequence,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};

use crate::{
    client::responses::{ContentItem, MessageRole, ResponseHistory, ResponseItem},
    execution::{ExecutionContinuation, ExecutionOutput},
    session::{SessionSnapshot, SessionSnapshotHead},
};

type Fields = BTreeMap<String, Box<RawValue>>;

fn fields(json: &str) -> Result<Fields> {
    serde_json::from_str(json).map_err(Error::InvalidPayload)
}

fn raw<T: Serialize + ?Sized>(value: &T) -> Result<Box<RawValue>> {
    serde_json::value::to_raw_value(value).map_err(Error::InvalidPayload)
}

pub(crate) fn prepare_receipt(json: &str) -> Result<EncodedPayload> {
    let mut receipt = fields(json)?;
    let Some(mut response) = receipt
        .get("response")
        .map(|response| fields(response.get()))
        .transpose()?
    else {
        return Ok(EncodedPayload::from_json(json.into()));
    };
    let Some(items) = response.remove("output_items") else {
        return Ok(EncodedPayload::from_json(json.into()));
    };
    let items: Vec<Box<RawValue>> =
        serde_json::from_str(items.get()).map_err(Error::InvalidPayload)?;
    let records = items
        .iter()
        .map(|item| EncodedPayload::from_json(item.get().into()))
        .collect::<Vec<_>>();
    let keys = records
        .iter()
        .map(EncodedPayload::reference)
        .collect::<Vec<_>>();
    response.insert("output_records".into(), raw(&keys)?);
    receipt.insert("response".into(), raw(&response)?);
    Ok(EncodedPayload::encode(&receipt)?.with_payloads(records))
}

pub(crate) async fn restore_receipt(reader: PayloadReader<'_>, json: String) -> Result<String> {
    let mut receipt = fields(&json)?;
    let Some(mut response) = receipt
        .get("response")
        .map(|response| fields(response.get()))
        .transpose()?
    else {
        return Ok(json);
    };
    let Some(keys) = response.remove("output_records") else {
        return Ok(json);
    };
    let keys: Vec<EncodedPayload> =
        serde_json::from_str(keys.get()).map_err(Error::InvalidPayload)?;
    let items = reader
        .load_payloads(keys)
        .await?
        .iter()
        .map(|item| item.json().map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let items =
        RawValue::from_string(format!("[{}]", items.join(","))).map_err(Error::InvalidPayload)?;
    response.insert("output_items".into(), items);
    receipt.insert("response".into(), raw(&response)?);
    serde_json::to_string(&receipt).map_err(Error::InvalidPayload)
}

fn assistant_text(item: &ResponseItem) -> Option<String> {
    let ResponseItem::Message {
        role: MessageRole::Assistant,
        content,
        ..
    } = item
    else {
        return None;
    };
    Some(
        content
            .iter()
            .filter_map(|part| match part {
                ContentItem::OutputText { text, .. } => Some(&**text),
                _ => None,
            })
            .collect(),
    )
}

#[derive(Serialize)]
struct ReferencedOutput<'a> {
    final_record: EncodedPayload,
    usage: &'a TurnUsage,
}

fn completion_output(history: &ResponseHistory, output: &ExecutionOutput) -> Result<Box<RawValue>> {
    let referenced = history
        .iter_rev()
        .next()
        .filter(|item| {
            !output.final_message.is_empty()
                && assistant_text(item).as_deref() == Some(output.final_message.as_str())
        })
        .map(EncodedPayload::encode)
        .transpose()?;
    match referenced {
        Some(record) => raw(&ReferencedOutput {
            final_record: record.reference(),
            usage: &output.usage,
        }),
        None => raw(output),
    }
}

pub(crate) async fn restore_output(
    reader: PayloadReader<'_>,
    mut output: Value,
) -> Result<ExecutionOutput> {
    if let Some(Value::String(key)) = output
        .as_object_mut()
        .and_then(|fields| fields.remove("final_record"))
    {
        let item: ResponseItem = reader
            .load_payload(EncodedPayload::by_key(&key))
            .await?
            .decode()?;
        let text = assistant_text(&item).ok_or_else(|| {
            Error::InvalidState(format!(
                "Codex output record {key} is not an assistant message"
            ))
        })?;
        output["final_message"] = Value::String(text);
    }
    serde_json::from_value(output).map_err(Error::InvalidPayload)
}

#[derive(Deserialize, Serialize)]
struct Continuation {
    state: Box<RawValue>,
    history: Sequence,
    prefix: Sequence,
}

pub(crate) fn prepare_continuation(
    value: ExecutionContinuation,
    known: &HashSet<String>,
) -> Result<Prepared> {
    let mut writer = PagedWriter::new(known);
    let history = writer.sequence(&value.history)?;
    let prefix = writer.sequence(&value.prefix)?;
    let state = RawValue::from_string(value.state_json)
        .map_err(nanocode_durability::Error::InvalidPayload)?;
    writer.finish(&Continuation {
        state,
        history,
        prefix,
    })
}

#[derive(Default)]
pub(crate) struct ContinuationCache {
    history: ResponseHistory,
    history_log: SequenceLog,
    prefix: Arc<[ResponseItem]>,
    prefix_log: SequenceLog,
}

impl ContinuationCache {
    pub(crate) fn committed(&mut self) {
        self.history_log.release();
        self.prefix_log.release();
    }

    fn sync_history(&mut self, history: &ResponseHistory) -> Result<()> {
        let unchanged = self
            .history
            .shared_prefix_len(history)
            .min(self.history_log.len());
        self.history_log.sync(
            unchanged,
            history.iter_from(unchanged).map(EncodedPayload::encode),
        )?;
        self.history = history.clone();
        Ok(())
    }

    fn sync_prefix(&mut self, prefix: &Arc<[ResponseItem]>) -> Result<()> {
        if !Arc::ptr_eq(&self.prefix, prefix) || self.prefix_log.len() != prefix.len() {
            self.prefix_log
                .sync(0, prefix.iter().map(EncodedPayload::encode))?;
            self.prefix = Arc::clone(prefix);
        }
        Ok(())
    }
}

pub(crate) fn prepare_cached_snapshot(
    snapshot: SessionSnapshot,
    cache: &mut ContinuationCache,
) -> Result<EncodedPayload> {
    let (head, history, prefix) = snapshot.into_context_parts();
    cached_parts(head, &history, prefix.as_ref(), cache)
}

pub(crate) fn prepare_cached_completion(
    snapshot: SessionSnapshot,
    output: &ExecutionOutput,
    cache: &mut ContinuationCache,
) -> Result<(EncodedPayload, Box<RawValue>)> {
    let (head, history, prefix) = snapshot.into_context_parts();
    let stored = completion_output(&history, output)?;
    Ok((
        cached_parts(head, &history, prefix.as_ref(), cache)?,
        stored,
    ))
}

fn cached_parts(
    head: SessionSnapshotHead,
    history: &ResponseHistory,
    prefix: Option<&Arc<[ResponseItem]>>,
    cache: &mut ContinuationCache,
) -> Result<EncodedPayload> {
    cache.sync_history(history)?;
    if let Some(prefix) = prefix {
        cache.sync_prefix(prefix)?;
    }
    let payload = EncodedPayload::encode(&Snapshot {
        head,
        history: cache.history_log.sequence(),
        prefix: prefix.map(|_| cache.prefix_log.sequence()),
    })?;
    let mut children = cache.history_log.frontier().cloned().collect::<Vec<_>>();
    if prefix.is_some() {
        children.extend(cache.prefix_log.frontier().cloned());
    }
    Ok(payload.with_payloads(children))
}

pub(crate) fn prepare_cached_continuation(
    state_json: String,
    history: &ResponseHistory,
    prefix: &Arc<[ResponseItem]>,
    cache: &mut ContinuationCache,
) -> Result<EncodedPayload> {
    cache.sync_history(history)?;
    cache.sync_prefix(prefix)?;
    let state =
        RawValue::from_string(state_json).map_err(nanocode_durability::Error::InvalidPayload)?;
    let payload = EncodedPayload::encode(&Continuation {
        state,
        history: cache.history_log.sequence(),
        prefix: cache.prefix_log.sequence(),
    })?;
    Ok(payload.with_payloads(
        cache
            .history_log
            .frontier()
            .chain(cache.prefix_log.frontier())
            .cloned()
            .collect(),
    ))
}

pub(crate) async fn load_continuation(
    reader: PayloadReader<'_>,
    payload: EncodedPayload,
) -> Result<(ExecutionContinuation, HashSet<String>)> {
    let value: Continuation = payload.decode()?;
    let mut keys = HashSet::new();
    let continuation = ExecutionContinuation {
        state_json: value.state.get().to_owned(),
        history: load_sequence(reader, value.history, &mut keys).await?,
        prefix: load_sequence(reader, value.prefix, &mut keys).await?,
    };
    Ok((continuation, keys))
}

#[derive(Deserialize)]
struct ContinuationHistory {
    history: Sequence,
}

pub(crate) async fn continuation_history(
    reader: PayloadReader<'_>,
    continuation: serde_json::Value,
) -> Result<Vec<serde_json::Value>> {
    let value: ContinuationHistory =
        serde_json::from_value(continuation).map_err(nanocode_durability::Error::InvalidPayload)?;
    load_sequence(reader, value.history, &mut HashSet::new()).await
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Snapshot {
    head: SessionSnapshotHead,
    history: Sequence,
    prefix: Option<Sequence>,
}

pub(crate) fn prepare_snapshot(
    value: SessionSnapshot,
    known: &HashSet<String>,
) -> Result<Prepared> {
    let (head, history, prefix) = value.into_context_parts();
    prepare_parts(head, &history, prefix.as_deref(), known)
}

fn prepare_parts(
    head: SessionSnapshotHead,
    history: &ResponseHistory,
    prefix: Option<&[ResponseItem]>,
    known: &HashSet<String>,
) -> Result<Prepared> {
    let mut writer = PagedWriter::new(known);
    let history = writer.sequence_iter(history)?;
    let prefix = prefix.map(|items| writer.sequence(items)).transpose()?;
    writer.finish(&Snapshot {
        head,
        history,
        prefix,
    })
}

pub(crate) async fn load_snapshot(
    reader: PayloadReader<'_>,
    payload: EncodedPayload,
) -> Result<SessionSnapshot> {
    restore_snapshot(reader, payload.decode()?).await
}

pub(crate) async fn restore_snapshot(
    reader: PayloadReader<'_>,
    saved: Snapshot,
) -> Result<SessionSnapshot> {
    load_snapshot_with_keys(reader, saved)
        .await
        .map(|(snapshot, _)| snapshot)
}

pub(crate) fn preloaded_snapshot(
    checkpoint: serde_json::Value,
    records: Option<Arc<HashSet<String>>>,
    key: &str,
) -> Option<(SessionSnapshot, HashSet<String>)> {
    let snapshot = SessionSnapshot::deserialize(checkpoint).ok()?;
    if let Some(records) = records {
        return Some((snapshot, Arc::unwrap_or_clone(records)));
    }
    let (head, history, prefix) = snapshot.into_context_parts();
    let known = HashSet::new();
    let mut writer = PagedWriter::new(&known);
    let saved = Snapshot {
        history: writer.sequence_iter(&history).ok()?,
        prefix: prefix
            .as_ref()
            .map(|items| writer.sequence(items))
            .transpose()
            .ok()?,
        head,
    };
    let prepared = writer.finish(&saved).ok()?;
    if !prepared.payload.matches_key(key) {
        return None;
    }
    Some((saved.head.with_context(history, prefix), prepared.keys))
}

pub(crate) async fn load_snapshot_with_keys(
    reader: PayloadReader<'_>,
    saved: Snapshot,
) -> Result<(SessionSnapshot, HashSet<String>)> {
    let mut keys = HashSet::new();
    let history = load_sequence(reader, saved.history, &mut keys).await?;
    let prefix = match saved.prefix {
        Some(value) => Some(load_sequence(reader, value, &mut keys).await?),
        None => None,
    };
    Ok((
        saved
            .head
            .with_context(ResponseHistory::new(history), prefix.map(Arc::from)),
        keys,
    ))
}
