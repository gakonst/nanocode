use std::collections::BTreeMap;

use nanocode_durability_core::StoreError;
use serde_json::value::RawValue;

use crate::{EffectClass, Outcome, Record, Status};

#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub payload: String,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub at: u64,
    pub event: Box<RawValue>,
    pub seq: u64,
    pub turn: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OpenCall {
    pub effect: EffectClass,
    pub input: Box<RawValue>,
    pub tool: String,
    pub turn: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Retry {
    pub attempts: u64,
    pub error: String,
    pub kind: String,
    pub retry_at: u64,
}

#[derive(Clone, Debug)]
pub enum Phase {
    Finished {
        last: u64,
        outcome: Outcome,
        result: Box<RawValue>,
    },
    Queued {
        retry: Option<Retry>,
    },
    Running {
        cancelling: bool,
        note: Option<String>,
        retry: Option<Retry>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Recovered {
    pub at: Option<u64>,
    pub attempt: u64,
}

#[derive(Clone, Debug)]
pub struct Turn {
    pub digest: String,
    pub edits: BTreeMap<String, String>,
    pub first: u64,
    pub hand: Option<String>,
    pub id: String,
    pub metadata: Box<RawValue>,
    pub phase: Phase,
    pub prompt: Box<RawValue>,
    pub recovered: Option<Recovered>,
}

#[derive(Clone, Debug, Default)]
pub struct Fold {
    calls: BTreeMap<String, OpenCall>,
    cells: BTreeMap<(String, u64), Vec<Box<RawValue>>>,
    checkpoints: BTreeMap<String, Checkpoint>,
    events: Vec<Event>,
    hosts: BTreeMap<String, Box<RawValue>>,
    index: BTreeMap<String, usize>,
    last_event: u64,
    models: BTreeMap<String, Vec<Box<RawValue>>>,
    seq: u64,
    stored: BTreeMap<String, String>,
    turns: Vec<Turn>,
}

fn invalid(detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("turn log record is invalid: {detail}"))
}

impl Fold {
    pub fn load(&mut self, seq: u64, body: &str) -> Result<(), StoreError> {
        if seq <= self.seq {
            return Err(invalid(format!(
                "row {seq} does not follow row {}",
                self.seq
            )));
        }
        for record in crate::decode(body)? {
            self.apply(&record, Some(seq))?;
        }
        self.seq = seq;
        Ok(())
    }

    pub(crate) const fn committed(&mut self, seq: u64) {
        self.seq = seq;
    }

    pub(crate) fn recovered_at(&mut self, turn: &str, seq: u64) {
        if let Some(turn) = self.turn_mut(turn)
            && let Some(recovered) = &mut turn.recovered
        {
            recovered.at = Some(seq);
        }
    }

    pub(crate) const fn reopen(&mut self, gap: u64) {
        self.last_event = self.last_event.saturating_add(gap);
    }

    pub(crate) const fn next_event(&mut self) -> u64 {
        self.last_event += 1;
        self.last_event
    }

    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    #[must_use]
    pub const fn last_event(&self) -> u64 {
        self.last_event
    }

    #[must_use]
    pub fn turn(&self, id: &str) -> Option<&Turn> {
        self.index.get(id).map(|&position| &self.turns[position])
    }

    fn turn_mut(&mut self, id: &str) -> Option<&mut Turn> {
        self.index
            .get(id)
            .copied()
            .map(|position| &mut self.turns[position])
    }

    #[must_use]
    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    #[must_use]
    pub fn checkpoint(&self, state: &str) -> Option<&Checkpoint> {
        self.checkpoints.get(state)
    }

    pub fn checkpoints(&self) -> impl Iterator<Item = (&str, &Checkpoint)> {
        self.checkpoints
            .iter()
            .map(|(state, checkpoint)| (state.as_str(), checkpoint))
    }

    #[must_use]
    pub fn stored(&self, key: &str) -> Option<&str> {
        self.stored.get(key).map(String::as_str)
    }

    #[must_use]
    pub fn host(&self, key: &str) -> Option<&RawValue> {
        self.hosts.get(key).map(AsRef::as_ref)
    }

    pub fn cells(&self) -> impl Iterator<Item = (&str, u64, &[Box<RawValue>])> {
        self.cells
            .iter()
            .map(|((session, cell), updates)| (session.as_str(), *cell, updates.as_slice()))
    }

    #[must_use]
    pub fn models(&self, turn: &str) -> &[Box<RawValue>] {
        self.models.get(turn).map_or(&[], Vec::as_slice)
    }

    pub fn open_calls(&self) -> impl Iterator<Item = (&str, &OpenCall)> {
        self.calls.iter().map(|(id, call)| (id.as_str(), call))
    }

    #[must_use]
    pub fn unknown(&self) -> Vec<String> {
        self.calls
            .iter()
            .filter(|(_, call)| call.effect.needs_row())
            .map(|(id, _)| id.clone())
            .collect()
    }

    #[must_use]
    pub fn recovering(&self) -> bool {
        self.turns.iter().any(|turn| {
            turn.recovered
                .is_some_and(|recovered| recovered.at == Some(self.seq))
        })
    }

    #[must_use]
    pub fn next_attempt(&self, turn: &str) -> u64 {
        match self.turn(turn).and_then(|turn| turn.recovered) {
            Some(Recovered {
                at: Some(at),
                attempt,
            }) if self.seq <= at => attempt + 1,
            Some(Recovered { at: None, attempt }) => attempt + 1,
            _ => 1,
        }
    }

    pub fn apply(&mut self, record: &Record, row: Option<u64>) -> Result<(), StoreError> {
        match record {
            Record::Admitted {
                digest,
                first,
                hand,
                metadata,
                prompt,
                turn,
            } => {
                if self.index.contains_key(turn) {
                    return Err(invalid(format!("turn {turn} was admitted twice")));
                }
                self.index.insert(turn.clone(), self.turns.len());
                self.turns.push(Turn {
                    digest: digest.clone(),
                    edits: BTreeMap::new(),
                    first: *first,
                    hand: hand.clone(),
                    id: turn.clone(),
                    metadata: metadata.clone(),
                    phase: Phase::Queued { retry: None },
                    prompt: prompt.clone(),
                    recovered: None,
                });
            }
            Record::Call {
                call_id,
                effect,
                input,
                tool,
                turn,
            } => {
                self.calls.insert(
                    call_id.clone(),
                    OpenCall {
                        effect: *effect,
                        input: input.clone(),
                        tool: tool.clone(),
                        turn: turn.clone(),
                    },
                );
            }
            Record::Cell {
                cell,
                fresh,
                session,
                update,
            } => {
                let updates = self.cells.entry((session.clone(), *cell)).or_default();
                if *fresh {
                    updates.clear();
                }
                updates.push(update.clone());
            }
            Record::Checkpoint {
                payload,
                revision,
                state,
            } => {
                self.checkpoints.insert(
                    state.clone(),
                    Checkpoint {
                        payload: payload.clone(),
                        revision: *revision,
                    },
                );
            }
            Record::Event {
                at,
                event,
                seq,
                turn,
            } => {
                if let Some(last) = self.events.last()
                    && *seq <= last.seq
                {
                    return Err(invalid(format!(
                        "event {seq} does not follow event {}",
                        last.seq
                    )));
                }
                self.last_event = self.last_event.max(*seq);
                self.events.push(Event {
                    at: *at,
                    event: event.clone(),
                    seq: *seq,
                    turn: turn.clone(),
                });
            }
            Record::Floor { seq } => {
                self.last_event = self.last_event.max(*seq);
            }
            Record::Forget { cell, session } => {
                self.cells.remove(&(session.clone(), *cell));
            }
            Record::Host { key, value } => {
                self.hosts.insert(key.clone(), value.clone());
            }
            Record::Model { response, turn } => {
                self.models
                    .entry(turn.clone())
                    .or_default()
                    .push(response.clone());
            }
            Record::Recovery {
                at,
                attempt,
                turn,
                unknown,
            } => {
                for call_id in unknown {
                    self.calls.remove(call_id);
                }
                let Some(found) = self.turn_mut(turn) else {
                    return Err(invalid(format!("recovery of unknown turn {turn}")));
                };
                found.recovered = Some(Recovered {
                    at: at.or(row),
                    attempt: *attempt,
                });
            }
            Record::Result { call_id, .. } => {
                self.calls.remove(call_id);
            }
            Record::Status { status, turn } => self.status(turn, status)?,
            Record::Stored { key, value } => {
                self.stored.insert(key.clone(), value.clone());
            }
        }
        Ok(())
    }

    fn status(&mut self, id: &str, status: &Status) -> Result<(), StoreError> {
        let Some(turn) = self.turn_mut(id) else {
            return Err(invalid(format!("status of unknown turn {id}")));
        };
        if let Phase::Finished { .. } = turn.phase {
            return Err(invalid(format!("status of finished turn {id}")));
        }
        match status {
            Status::Cancelling => {
                if let Phase::Running { cancelling, .. } = &mut turn.phase {
                    *cancelling = true;
                }
            }
            Status::Finished {
                last,
                outcome,
                result,
            } => {
                turn.phase = Phase::Finished {
                    last: *last,
                    outcome: *outcome,
                    result: result.clone(),
                };
                self.calls
                    .retain(|_, call| call.turn.as_deref() != Some(id));
            }
            Status::Interrupted { note: text } => {
                if let Phase::Running { note, .. } = &mut turn.phase {
                    *note = Some(text.clone());
                }
            }
            Status::Replaced {
                digest,
                edit,
                edit_id,
                prompt,
            } => {
                turn.digest.clone_from(digest);
                turn.prompt = prompt.clone();
                turn.edits.insert(edit_id.clone(), edit.clone());
            }
            Status::Retrying {
                attempts,
                error,
                kind,
                retry_at,
            } => {
                let next = Some(Retry {
                    attempts: *attempts,
                    error: error.clone(),
                    kind: kind.clone(),
                    retry_at: *retry_at,
                });
                match &mut turn.phase {
                    Phase::Queued { retry } | Phase::Running { retry, .. } => *retry = next,
                    Phase::Finished { .. } => {}
                }
            }
            Status::Running => {
                turn.phase = Phase::Running {
                    cancelling: false,
                    note: None,
                    retry: None,
                };
            }
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<String, StoreError> {
        let mut records = Vec::new();
        for (key, value) in &self.stored {
            records.push(Record::Stored {
                key: key.clone(),
                value: value.clone(),
            });
        }
        for (state, checkpoint) in &self.checkpoints {
            records.push(Record::Checkpoint {
                payload: checkpoint.payload.clone(),
                revision: checkpoint.revision,
                state: state.clone(),
            });
        }
        for turn in &self.turns {
            records.push(Record::Admitted {
                digest: turn.digest.clone(),
                first: turn.first,
                hand: turn.hand.clone(),
                metadata: turn.metadata.clone(),
                prompt: turn.prompt.clone(),
                turn: turn.id.clone(),
            });
            let status = |status| Record::Status {
                status,
                turn: turn.id.clone(),
            };
            for (edit_id, edit) in &turn.edits {
                records.push(status(Status::Replaced {
                    digest: turn.digest.clone(),
                    edit: edit.clone(),
                    edit_id: edit_id.clone(),
                    prompt: turn.prompt.clone(),
                }));
            }
            match &turn.phase {
                Phase::Queued { retry } => {
                    records.extend(retry.as_ref().map(|retry| status(retrying(retry))));
                }
                Phase::Running {
                    cancelling,
                    note,
                    retry,
                } => {
                    records.push(status(Status::Running));
                    if *cancelling {
                        records.push(status(Status::Cancelling));
                    }
                    if let Some(note) = note {
                        records.push(status(Status::Interrupted { note: note.clone() }));
                    }
                    records.extend(retry.as_ref().map(|retry| status(retrying(retry))));
                }
                Phase::Finished {
                    last,
                    outcome,
                    result,
                } => records.push(status(Status::Finished {
                    last: *last,
                    outcome: *outcome,
                    result: result.clone(),
                })),
            }
            if let Some(recovered) = turn.recovered {
                records.push(Record::Recovery {
                    at: recovered.at,
                    attempt: recovered.attempt,
                    turn: turn.id.clone(),
                    unknown: Vec::new(),
                });
            }
            for response in self.models(&turn.id) {
                records.push(Record::Model {
                    response: response.clone(),
                    turn: turn.id.clone(),
                });
            }
        }
        for (call_id, call) in &self.calls {
            records.push(Record::Call {
                call_id: call_id.clone(),
                effect: call.effect,
                input: call.input.clone(),
                tool: call.tool.clone(),
                turn: call.turn.clone(),
            });
        }
        for ((session, cell), updates) in &self.cells {
            for (position, update) in updates.iter().enumerate() {
                records.push(Record::Cell {
                    cell: *cell,
                    fresh: position == 0,
                    session: session.clone(),
                    update: update.clone(),
                });
            }
        }
        for (key, value) in &self.hosts {
            records.push(Record::Host {
                key: key.clone(),
                value: value.clone(),
            });
        }
        for event in &self.events {
            records.push(Record::Event {
                at: event.at,
                event: event.event.clone(),
                seq: event.seq,
                turn: event.turn.clone(),
            });
        }
        records.push(Record::Floor {
            seq: self.last_event,
        });
        crate::encode(&records)
    }
}

fn retrying(retry: &Retry) -> Status {
    Status::Retrying {
        attempts: retry.attempts,
        error: retry.error.clone(),
        kind: retry.kind.clone(),
        retry_at: retry.retry_at,
    }
}
