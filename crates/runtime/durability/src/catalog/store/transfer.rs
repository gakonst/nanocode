use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use nanocode_agent_core::Origin;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    BranchPoint, SessionStore, SessionSummary, settled_checkpoint, summarize, unknown_turn,
};
use crate::{
    BlobKey, BlobUrl, BranchBoundary, DurableState, EncodedPayload, Error, OwnerId, OwnerToken,
    PagedWriter, PayloadReader, Result, Sequence, SessionRecord, StateCommit, StateStore,
    StoreRecord, StoredState, SummaryUpdate, Transition, catalog::Importing, load_sequence,
    session::reduce_peeked, shared_store::SharedStore,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchExport {
    pub mode: BranchExportMode,
    pub record: SessionRecord,
    pub boundary: BranchBoundary,
    pub checkpoint: Option<String>,
    pub inherited: String,
    pub turns: u64,
    pub preview: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchExportMode {
    AllTurns,
    Checkpoint,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BranchCursor {
    pub entry: u64,
    pub offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BranchPage {
    pub cursor: BranchCursor,
    pub next: Option<BranchCursor>,
    pub records: Vec<StoreRecord>,
}

pub struct BranchImport {
    sessions: SessionStore,
    session_id: String,
    stage: Stage,
}

enum Stage {
    Staged {
        owner: OwnerToken,
        state: DurableState,
    },
    Published(SessionSummary),
}

enum Existing {
    Absent,
    Staged(DurableState),
    Published(SessionRecord),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InheritedTurn {
    turn: String,
    checkpoint: Option<EncodedPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before: Option<EncodedPayload>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Start {
    Empty,
    Checkpoint(EncodedPayload),
    Unavailable,
}

impl Start {
    fn checkpoint(&self) -> Result<Option<EncodedPayload>> {
        match self {
            Self::Empty => Ok(None),
            Self::Checkpoint(checkpoint) => Ok(Some(checkpoint.clone())),
            Self::Unavailable => Err(Error::InvalidState(
                "retained history before the selected turn was pruned".into(),
            )),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Inherited {
    start: Start,
    turns: Sequence,
}

fn squashed_index(checkpoint: &EncodedPayload) -> Result<EncodedPayload> {
    EncodedPayload::encode(&Inherited {
        start: Start::Checkpoint(checkpoint.reference()),
        turns: Sequence::default(),
    })
}

struct History {
    start: Start,
    own_start: Start,
    inherited: usize,
    turns: Vec<InheritedTurn>,
}

impl History {
    fn select(
        &self,
        state: &DurableState,
        at: &BranchPoint,
    ) -> Result<(Option<EncodedPayload>, usize)> {
        let position = |turn: &str| self.turns.iter().position(|entry| entry.turn == turn);
        match at {
            BranchPoint::Latest => {
                if !state.pending_operations().is_empty() {
                    return Err(Error::InvalidState(
                        "the latest checkpoint is not settled while operations are pending; export through a settled turn instead".into(),
                    ));
                }
                Ok((
                    state.latest_checkpoint().map(EncodedPayload::reference),
                    self.turns.len(),
                ))
            }
            BranchPoint::Through(turn) => {
                let Some(position) = position(turn) else {
                    return Err(if state.operation(turn).is_some() {
                        no_settled_checkpoint()
                    } else {
                        unknown_turn()
                    });
                };
                let checkpoint = self.turns[position]
                    .checkpoint
                    .clone()
                    .ok_or_else(no_settled_checkpoint)?;
                Ok((Some(checkpoint), position + 1))
            }
            BranchPoint::Before(turn) => {
                let covered = match position(turn) {
                    Some(position) => position,
                    None if state
                        .first_pending_operation()
                        .is_some_and(|(pending, _)| pending == turn) =>
                    {
                        self.turns.len()
                    }
                    None if state.operation(turn).is_some() => {
                        return Err(Error::InvalidState(
                            "the turn before the selected one has not settled".into(),
                        ));
                    }
                    None => return Err(unknown_turn()),
                };
                let before = self.turns.get(covered).and_then(|turn| turn.before.clone());
                let checkpoint = if covered == self.inherited {
                    self.own_start.checkpoint()?
                } else if before.is_some() {
                    before
                } else if covered == 0 {
                    self.start.checkpoint()?
                } else {
                    Some(self.turns[covered - 1].checkpoint.clone().ok_or_else(|| {
                        Error::InvalidState(
                            "checkpoint before the selected turn is unavailable".into(),
                        )
                    })?)
                };
                Ok((checkpoint, covered))
            }
        }
    }

    fn boundary(&self, id: &str, covered: usize) -> BranchBoundary {
        BranchBoundary {
            source_session_id: id.to_owned(),
            through_turn: self.turns[..covered].last().map(|turn| turn.turn.clone()),
        }
    }

    fn index(&self, covered: usize) -> Result<EncodedPayload> {
        let known = HashSet::new();
        let mut writer = PagedWriter::new(&known);
        let turns = writer.sequence(&self.turns[..covered])?;
        Ok(writer
            .finish(&Inherited {
                start: self.start.clone(),
                turns,
            })?
            .payload)
    }

    fn roots(&self, covered: usize, selected: Option<&str>) -> Vec<String> {
        checkpoints(&self.start, &self.turns[..covered])
            .map(|checkpoint| checkpoint.key().to_owned())
            .chain(selected.map(str::to_owned))
            .collect()
    }
}

fn checkpoints<'a>(
    start: &'a Start,
    turns: &'a [InheritedTurn],
) -> impl Iterator<Item = &'a EncodedPayload> {
    let start = match start {
        Start::Checkpoint(checkpoint) => Some(checkpoint),
        Start::Empty | Start::Unavailable => None,
    };
    start.into_iter().chain(
        turns
            .iter()
            .flat_map(|turn| turn.before.iter().chain(&turn.checkpoint)),
    )
}

#[derive(Default)]
struct Closure {
    edges: HashMap<String, Vec<String>>,
    absent: HashSet<String>,
    blobs: BTreeSet<BlobKey>,
}

impl Closure {
    async fn expand(&mut self, store: &SharedStore, id: &str, root: &str) -> Result<()> {
        let mut pending = vec![root.to_owned()];
        while !pending.is_empty() {
            let mut unknown: Vec<String> = pending
                .drain(..)
                .filter(|key| !self.edges.contains_key(key) && !self.absent.contains(key))
                .collect();
            unknown.sort_unstable();
            unknown.dedup();
            if unknown.is_empty() {
                break;
            }
            let values = store.clone().read_records(id, &unknown).await?;
            if values.len() != unknown.len() {
                return Err(Error::InvalidState("record batch length mismatch".into()));
            }
            for (key, value) in unknown.into_iter().zip(values) {
                let Some(value) = value else {
                    self.absent.insert(key);
                    continue;
                };
                let edges = match value.strip_prefix('=') {
                    Some(content) => references(&key, content, &mut self.blobs)?,
                    None => {
                        let mut parts = HashSet::new();
                        let payload = EncodedPayload::by_key(&key)
                            .load_noting(&mut store.clone(), id, &mut parts)
                            .await?;
                        parts.remove(&key);
                        let mut edges: Vec<String> = parts.into_iter().collect();
                        for part in &edges {
                            self.edges.entry(part.clone()).or_default();
                        }
                        edges.extend(references(&key, payload.json()?, &mut self.blobs)?);
                        edges
                    }
                };
                pending.extend(
                    edges
                        .iter()
                        .filter(|edge| !self.edges.contains_key(*edge))
                        .cloned(),
                );
                self.edges.insert(key, edges);
            }
        }
        if self.absent.contains(root) {
            return Err(Error::InvalidState(format!(
                "missing payload record {root}"
            )));
        }
        Ok(())
    }

    fn reach(&self, root: &str, skip: &HashSet<String>) -> HashSet<String> {
        let mut seen = HashSet::new();
        let mut pending = vec![root];
        while let Some(key) = pending.pop() {
            if skip.contains(key) || self.absent.contains(key) || !seen.insert(key.to_owned()) {
                continue;
            }
            if let Some(edges) = self.edges.get(key) {
                pending.extend(edges.iter().map(String::as_str));
            }
        }
        seen
    }
}

fn references(key: &str, content: &str, blobs: &mut BTreeSet<BlobKey>) -> Result<Vec<String>> {
    fn visit(value: &Value, found: &mut Vec<String>, blobs: &mut BTreeSet<BlobKey>) {
        match value {
            Value::String(text) if EncodedPayload::is_key(text) => found.push(text.clone()),
            Value::String(text) => {
                if let Some(blob) = BlobUrl::parse(text) {
                    blobs.insert(blob.key().clone());
                }
            }
            Value::Array(items) => items.iter().for_each(|item| visit(item, found, blobs)),
            Value::Object(fields) => fields.values().for_each(|item| visit(item, found, blobs)),
            _ => {}
        }
    }
    let value: Value = serde_json::from_str(content).map_err(|error| {
        Error::InvalidState(format!("record {key} does not hold JSON: {error}"))
    })?;
    let mut found = Vec::new();
    visit(&value, &mut found, blobs);
    found.retain(|reference| reference != key);
    Ok(found)
}

fn no_settled_checkpoint() -> Error {
    Error::InvalidState("the selected turn has no settled checkpoint".into())
}

fn history_changed() -> Error {
    Error::InvalidState(
        "the source session's history through the exported boundary changed after the export"
            .into(),
    )
}

fn is_branch(record: &SessionRecord, export: &BranchExport) -> bool {
    record.branch.as_ref() == Some(&export.boundary)
        && record.start_checkpoint == export.checkpoint
        && record.inherited.as_deref() == Some(export.inherited.as_str())
}

fn importing(state: &DurableState) -> Result<&Importing> {
    state
        .session()
        .and_then(|record| record.importing.as_ref())
        .ok_or_else(|| Error::InvalidState("the staged branch import lost its progress".into()))
}

fn successor(state: &DurableState) -> Result<u64> {
    state
        .revision()
        .checked_add(1)
        .ok_or_else(|| Error::InvalidState("state revision exceeded the u64 range".to_owned()))
}

impl SessionStore {
    pub async fn export_branch(&self, id: &str, at: BranchPoint) -> Result<BranchExport> {
        self.export(id, at, BranchExportMode::AllTurns).await
    }

    pub async fn export_squashed_branch(&self, id: &str, at: BranchPoint) -> Result<BranchExport> {
        self.export(id, at, BranchExportMode::Checkpoint).await
    }

    async fn export(
        &self,
        id: &str,
        at: BranchPoint,
        mode: BranchExportMode,
    ) -> Result<BranchExport> {
        let state = self.require(id).await?;
        let record = self.source_record(id, &state).await?;
        let history = self.history(id, &state).await?;
        let (checkpoint, covered) = history.select(&state, &at)?;
        let index = match mode {
            BranchExportMode::AllTurns => history.index(covered)?,
            BranchExportMode::Checkpoint => {
                squashed_index(checkpoint.as_ref().ok_or_else(no_settled_checkpoint)?)?
            }
        };
        let preview = match at {
            BranchPoint::Before(_) => None,
            _ => self.first_prompt(id, &state).await?,
        };
        Ok(BranchExport {
            mode,
            record,
            boundary: history.boundary(id, covered),
            checkpoint: checkpoint.map(|checkpoint| checkpoint.key().to_owned()),
            inherited: index.key().to_owned(),
            turns: u64::try_from(covered).map_err(|_| history_changed())?,
            preview,
        })
    }

    pub async fn export_page(
        &self,
        id: &str,
        export: &BranchExport,
        cursor: BranchCursor,
        max_bytes: usize,
    ) -> Result<BranchPage> {
        if max_bytes == 0 {
            return Err(Error::InvalidState(
                "a branch page needs a positive byte budget".into(),
            ));
        }
        let (mut index, roots) = self.exported(id, export).await?;
        let mut index_records = Vec::new();
        index.stage(&mut index_records);
        index_records.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        index_records.dedup_by(|a, b| a.key == b.key);
        let invalid_cursor = || {
            Error::InvalidState(format!(
                "branch cursor at entry {} offset {} is outside the export",
                cursor.entry, cursor.offset
            ))
        };
        let mut entry = usize::try_from(cursor.entry)
            .ok()
            .filter(|entry| *entry <= roots.len())
            .ok_or_else(invalid_cursor)?;
        let mut offset = usize::try_from(cursor.offset).map_err(|_| invalid_cursor())?;
        let mut closure = Closure::default();
        if let Some(previous) = entry.checked_sub(1).filter(|_| entry < roots.len()) {
            closure.expand(&self.store, id, &roots[previous]).await?;
        }
        let mut records = Vec::new();
        let mut bytes = 0;
        loop {
            if !records.is_empty() && bytes >= max_bytes {
                return Ok(BranchPage {
                    cursor,
                    next: Some(BranchCursor::at(entry, offset)?),
                    records,
                });
            }
            let remaining = if let Some(root) = roots.get(entry) {
                closure.expand(&self.store, id, root).await?;
                let previous = match entry.checked_sub(1) {
                    Some(previous) => closure.reach(&roots[previous], &HashSet::new()),
                    None => HashSet::new(),
                };
                let mut fresh: Vec<String> = closure.reach(root, &previous).into_iter().collect();
                fresh.sort_unstable();
                if offset > fresh.len() {
                    return Err(invalid_cursor());
                }
                let keys = fresh.split_off(offset);
                let values = self.store.clone().read_records(id, &keys).await?;
                if values.len() != keys.len() {
                    return Err(Error::InvalidState("record batch length mismatch".into()));
                }
                keys.into_iter()
                    .zip(values)
                    .map(|(key, value)| {
                        value
                            .map(|value| StoreRecord {
                                key: key.clone(),
                                value,
                            })
                            .ok_or_else(|| {
                                Error::InvalidState(format!("missing payload record {key}"))
                            })
                    })
                    .collect::<Result<Vec<_>>>()?
            } else {
                index_records
                    .get(offset..)
                    .ok_or_else(invalid_cursor)?
                    .to_vec()
            };
            for record in remaining {
                let size = record.key.len() + record.value.len();
                if !records.is_empty() && bytes + size > max_bytes {
                    return Ok(BranchPage {
                        cursor,
                        next: Some(BranchCursor::at(entry, offset)?),
                        records,
                    });
                }
                bytes += size;
                records.push(record);
                offset += 1;
            }
            if entry == roots.len() {
                return Ok(BranchPage {
                    cursor,
                    next: None,
                    records,
                });
            }
            entry += 1;
            offset = 0;
        }
    }

    pub async fn export_blobs(&self, id: &str, export: &BranchExport) -> Result<Vec<BlobKey>> {
        let (_, roots) = self.exported(id, export).await?;
        let mut closure = Closure::default();
        for root in &roots {
            closure.expand(&self.store, id, root).await?;
        }
        Ok(closure.blobs.into_iter().collect())
    }

    async fn exported(
        &self,
        id: &str,
        export: &BranchExport,
    ) -> Result<(EncodedPayload, Vec<String>)> {
        let (index, roots) = match export.mode {
            BranchExportMode::AllTurns => {
                let state = self.require(id).await?;
                let history = self.history(id, &state).await?;
                let covered = usize::try_from(export.turns)
                    .ok()
                    .filter(|covered| *covered <= history.turns.len())
                    .ok_or_else(history_changed)?;
                (
                    history.index(covered)?,
                    history.roots(covered, export.checkpoint.as_deref()),
                )
            }
            BranchExportMode::Checkpoint => {
                let checkpoint = export
                    .checkpoint
                    .as_deref()
                    .ok_or_else(no_settled_checkpoint)?;
                (
                    squashed_index(&EncodedPayload::from_key(checkpoint))?,
                    vec![checkpoint.to_owned()],
                )
            }
        };
        if index.key() != export.inherited {
            return Err(history_changed());
        }
        Ok((index, roots))
    }

    pub async fn import_branch(
        &self,
        session_id: &str,
        export: &BranchExport,
    ) -> Result<BranchImport> {
        let mut store = self.store.clone();
        let stage = match Self::existing(session_id, store.peek(session_id).await?, export)? {
            Existing::Published(record) => Stage::Published(SessionSummary {
                record,
                preview: export.preview.clone(),
            }),
            Existing::Absent | Existing::Staged(_) => {
                let owned = store.acquire(session_id, OwnerId::new()).await?;
                match Self::existing(session_id, owned.state, export)? {
                    Existing::Published(record) => Stage::Published(SessionSummary {
                        record,
                        preview: export.preview.clone(),
                    }),
                    Existing::Staged(state) => Stage::Staged {
                        owner: owned.owner,
                        state,
                    },
                    Existing::Absent => {
                        let mut record = export.record.derive(session_id, Origin::Branch);
                        record.branch = Some(export.boundary.clone());
                        record.start_checkpoint.clone_from(&export.checkpoint);
                        record.inherited = Some(export.inherited.clone());
                        record.importing = Some(Importing {
                            preview: export.preview.clone(),
                            next: Some(BranchCursor::default()),
                        });
                        let mut state = DurableState::default();
                        state.advance_revision(1)?;
                        state.set_session(Some(record));
                        store
                            .commit(
                                session_id,
                                &owned.owner,
                                0,
                                StateCommit {
                                    payload: &state.checkpoint_payload()?,
                                    records: &[],
                                    events: &[],
                                    summary: &SummaryUpdate::Unchanged,
                                },
                            )
                            .await?;
                        Stage::Staged {
                            owner: owned.owner,
                            state,
                        }
                    }
                }
            }
        };
        Ok(BranchImport {
            sessions: self.clone(),
            session_id: session_id.to_owned(),
            stage,
        })
    }

    fn existing(session_id: &str, stored: StoredState, export: &BranchExport) -> Result<Existing> {
        if stored.payload.is_none() && stored.revision == 0 {
            return Ok(Existing::Absent);
        }
        let state = reduce_peeked(stored)?;
        match state.session() {
            Some(record) if is_branch(record, export) && record.importing.is_some() => {
                Ok(Existing::Staged(state))
            }
            Some(record) if is_branch(record, export) => Ok(Existing::Published(record.clone())),
            _ => Err(Error::InvalidState(format!(
                "session {session_id} already exists and is not an import of this branch export"
            ))),
        }
    }

    pub(super) async fn inherited_point(
        &self,
        id: &str,
        state: &DurableState,
        at: &BranchPoint,
    ) -> Result<Option<(Option<EncodedPayload>, BranchBoundary)>> {
        let (BranchPoint::Through(turn) | BranchPoint::Before(turn)) = at else {
            return Ok(None);
        };
        if state.operation(turn).is_some()
            || state
                .session()
                .and_then(|record| record.inherited.as_ref())
                .is_none()
        {
            return Ok(None);
        }
        let history = self.history(id, state).await?;
        if !history.turns[..history.inherited]
            .iter()
            .any(|entry| entry.turn == *turn)
        {
            return Ok(None);
        }
        let (checkpoint, covered) = history.select(state, at)?;
        Ok(Some((checkpoint, history.boundary(id, covered))))
    }

    async fn history(&self, id: &str, state: &DurableState) -> Result<History> {
        let mut operations: Vec<_> = state
            .operations()
            .iter()
            .filter(|(_, operation)| operation.status.is_terminal())
            .collect();
        operations.sort_by_key(|(_, operation)| operation.accepted_order());
        let own = operations
            .into_iter()
            .map(|(turn, operation)| InheritedTurn {
                turn: turn.clone(),
                checkpoint: settled_checkpoint(&operation.status).map(EncodedPayload::reference),
                before: None,
            });
        let pruned = match state.session() {
            Some(record) => record.history_pruned,
            None => state
                .operations()
                .values()
                .map(|operation| operation.accepted_order())
                .min()
                .is_some_and(|order| order != 1),
        };
        if pruned {
            return Ok(History {
                start: Start::Unavailable,
                own_start: Start::Unavailable,
                inherited: 0,
                turns: own.collect(),
            });
        }
        let own_start = match Self::start_checkpoint(id, state) {
            Ok(Some((_, checkpoint))) => Start::Checkpoint(checkpoint.reference()),
            Ok(None) => Start::Empty,
            Err(_) => Start::Unavailable,
        };
        let (start, mut turns) = match state
            .session()
            .and_then(|record| record.inherited.as_deref())
        {
            Some(key) => self.inherited(id, key).await?,
            None => (own_start.clone(), Vec::new()),
        };
        let inherited = turns.len();
        let before = match (&own_start, turns.last()) {
            (Start::Checkpoint(checkpoint), Some(last))
                if last.checkpoint.as_ref() != Some(checkpoint) =>
            {
                Some(checkpoint.clone())
            }
            _ => None,
        };
        turns.extend(own);
        if let Some(first) = turns.get_mut(inherited) {
            first.before = before;
        }
        Ok(History {
            start,
            own_start,
            inherited,
            turns,
        })
    }

    async fn inherited(&self, id: &str, key: &str) -> Result<(Start, Vec<InheritedTurn>)> {
        let index: Inherited = EncodedPayload::from_key(key)
            .load(&mut self.store.clone(), id)
            .await?
            .decode()?;
        let turns = load_sequence(
            PayloadReader::store(&self.store, id),
            index.turns,
            &mut HashSet::new(),
        )
        .await?;
        Ok((index.start, turns))
    }
}

impl BranchCursor {
    fn at(entry: usize, offset: usize) -> Result<Self> {
        let overflow = || Error::InvalidState("branch cursor exceeded the u64 range".into());
        Ok(Self {
            entry: u64::try_from(entry).map_err(|_| overflow())?,
            offset: u64::try_from(offset).map_err(|_| overflow())?,
        })
    }
}

impl BranchImport {
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn next(&self) -> Option<BranchCursor> {
        match &self.stage {
            Stage::Staged { state, .. } => {
                importing(state).ok().and_then(|importing| importing.next)
            }
            Stage::Published(_) => None,
        }
    }

    pub async fn import(&mut self, page: &BranchPage) -> Result<()> {
        let Stage::Staged { owner, state } = &mut self.stage else {
            return Ok(());
        };
        let Some(expected) = importing(state)?.next else {
            return Ok(());
        };
        if page.cursor > expected {
            return Err(Error::InvalidState(format!(
                "branch page starts at entry {} offset {} but the import expects entry {} offset {}",
                page.cursor.entry, page.cursor.offset, expected.entry, expected.offset
            )));
        }
        if page.next.is_some_and(|next| next <= expected) {
            return Ok(());
        }
        let mut verified = BTreeMap::new();
        for record in &page.records {
            EncodedPayload::verify_record(&record.key, &record.value)?;
            if verified
                .insert(record.key.as_str(), record.value.as_str())
                .is_some_and(|value| value != record.value)
            {
                return Err(Error::InvalidState(format!(
                    "branch page holds record {} twice with different content",
                    record.key
                )));
            }
        }
        let records: Vec<StoreRecord> = verified
            .into_iter()
            .map(|(key, value)| StoreRecord {
                key: key.to_owned(),
                value: value.to_owned(),
            })
            .collect();
        let mut staged = state.clone();
        staged.advance_revision(successor(state)?)?;
        if let Some(importing) = staged
            .session_mut()
            .and_then(|record| record.importing.as_mut())
        {
            importing.next = page.next;
        }
        self.sessions
            .store
            .clone()
            .commit(
                &self.session_id,
                owner,
                state.revision(),
                StateCommit {
                    payload: &staged.checkpoint_payload()?,
                    records: &records,
                    events: &[],
                    summary: &SummaryUpdate::Unchanged,
                },
            )
            .await?;
        *state = staged;
        Ok(())
    }

    pub async fn publish(self) -> Result<SessionSummary> {
        let (owner, state) = match self.stage {
            Stage::Published(summary) => return Ok(summary),
            Stage::Staged { owner, state } => (owner, state),
        };
        let id = self.session_id.as_str();
        let mut record = state.session().cloned().ok_or_else(|| {
            Error::InvalidState("the staged branch import lost its record".into())
        })?;
        let importing = record.importing.take().ok_or_else(|| {
            Error::InvalidState("the staged branch import lost its progress".into())
        })?;
        if let Some(next) = importing.next {
            return Err(Error::InvalidState(format!(
                "the branch import still expects the page at entry {} offset {}",
                next.entry, next.offset
            )));
        }
        let checkpoint = record
            .start_checkpoint
            .as_deref()
            .map(EncodedPayload::from_key);
        if let Some(checkpoint) = &checkpoint {
            self.sessions
                .hydrate(id, checkpoint, Some(record.family()))
                .await?;
        }
        if let Some(key) = &record.inherited {
            let (start, turns) = self.sessions.inherited(id, key).await?;
            let roots: Vec<String> = checkpoints(&start, &turns)
                .map(|checkpoint| checkpoint.key().to_owned())
                .collect();
            let found = self.sessions.store.clone().read_records(id, &roots).await?;
            if found.len() != roots.len() {
                return Err(Error::InvalidState("record batch length mismatch".into()));
            }
            if let Some((missing, _)) = roots.iter().zip(&found).find(|(_, value)| value.is_none())
            {
                return Err(Error::InvalidState(format!(
                    "the imported branch is missing checkpoint record {missing}"
                )));
            }
        }
        let mut published = state.clone();
        match checkpoint {
            Some(checkpoint) => published.apply_transition(
                successor(&state)?,
                Transition::CheckpointCommitted { checkpoint },
            )?,
            None => published.advance_revision(successor(&state)?)?,
        }
        record.touch();
        published.set_session(Some(record.clone()));
        let mut store = self.sessions.store.clone();
        let summary = if store.supports_summaries() {
            match summarize(&published, importing.preview.clone())? {
                Some(summary) => SummaryUpdate::Set(summary),
                None => SummaryUpdate::Unchanged,
            }
        } else {
            SummaryUpdate::Unchanged
        };
        store
            .commit(
                id,
                &owner,
                state.revision(),
                StateCommit {
                    payload: &published.checkpoint_payload()?,
                    records: &published.stage_records(),
                    events: &[],
                    summary: &summary,
                },
            )
            .await?;
        Ok(SessionSummary {
            record,
            preview: importing.preview,
        })
    }
}
