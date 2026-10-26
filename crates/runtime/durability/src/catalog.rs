use std::{collections::BTreeMap, path::PathBuf};

use nanocode_agent_core::{HarnessFamily, Lineage, ModelSelection};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,

    pub model: ModelSelection,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    pub lineage: Lineage,

    #[serde(default)]
    pub created_at_ms: u64,

    #[serde(default)]
    pub updated_at_ms: u64,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub history_pruned: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<BranchBoundary>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) start_checkpoint: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<InitialSettings>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inherited_transcript: Vec<nanocode_agent_core::input::PromptMessage>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) has_events: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) event_preview: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) inherited: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) importing: Option<Importing>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Importing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preview: Option<String>,

    pub(crate) next: Option<BranchCursor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialSettings {
    pub thinking: nanocode_agent_core::Thinking,
    pub service_tier: nanocode_agent_core::ServiceTier,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchBoundary {
    pub source_session_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through_turn: Option<String>,
}

impl SessionRecord {
    #[must_use]
    pub fn root(
        session_id: impl Into<String>,
        model: ModelSelection,
        workspace: Option<PathBuf>,
    ) -> Self {
        let session_id = session_id.into();
        let now = now_ms();
        Self {
            lineage: Lineage::root(session_id.clone()),
            session_id,
            model,
            workspace,
            title: None,
            created_at_ms: now,
            updated_at_ms: now,
            history_pruned: false,
            branch: None,
            start_checkpoint: None,
            initial: None,
            inherited_transcript: Vec::new(),
            metadata: BTreeMap::new(),
            has_events: false,
            event_preview: None,
            inherited: None,
            importing: None,
        }
    }

    #[must_use]
    pub fn with_inherited_transcript(
        mut self,
        transcript: Vec<nanocode_agent_core::input::PromptMessage>,
    ) -> Self {
        self.inherited_transcript = transcript;
        self
    }

    #[must_use]
    pub fn family(&self) -> HarnessFamily {
        self.model.family()
    }

    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn with_lineage(mut self, lineage: Lineage) -> Self {
        self.lineage = lineage;
        self
    }

    #[must_use]
    pub fn derive(
        &self,
        session_id: impl Into<String>,
        origin: nanocode_agent_core::Origin,
    ) -> Self {
        let session_id = session_id.into();
        let now = now_ms();
        Self {
            lineage: Lineage::child_of(&self.lineage, self.session_id.clone(), origin),
            session_id,
            model: self.model.clone(),
            workspace: self.workspace.clone(),
            title: self.title.clone(),
            created_at_ms: now,
            updated_at_ms: now,
            history_pruned: false,
            branch: None,
            start_checkpoint: None,
            initial: None,
            inherited_transcript: Vec::new(),
            metadata: BTreeMap::new(),
            has_events: false,
            event_preview: None,
            inherited: None,
            importing: None,
        }
    }

    pub(crate) fn touch(&mut self) {
        let now = now_ms();
        if now > self.updated_at_ms {
            self.updated_at_ms = now;
        }
    }

    pub(crate) fn merged(&self, next: Self) -> Self {
        if self.importing.is_some() {
            return next;
        }
        let lineage = if next.lineage == Lineage::root(next.session_id.clone()) {
            self.lineage.clone()
        } else {
            next.lineage
        };
        Self {
            session_id: next.session_id,
            model: next.model,
            workspace: next.workspace.or_else(|| self.workspace.clone()),
            title: next.title.or_else(|| self.title.clone()),
            lineage,
            created_at_ms: if self.created_at_ms == 0 {
                next.created_at_ms
            } else {
                self.created_at_ms
            },
            updated_at_ms: self.updated_at_ms,
            history_pruned: self.history_pruned || next.history_pruned,

            branch: self.branch.clone().or(next.branch),
            start_checkpoint: next
                .start_checkpoint
                .or_else(|| self.start_checkpoint.clone()),
            initial: next.initial.or(self.initial),
            inherited_transcript: if next.inherited_transcript.is_empty() {
                self.inherited_transcript.clone()
            } else {
                next.inherited_transcript
            },
            metadata: if next.metadata.is_empty() {
                self.metadata.clone()
            } else {
                next.metadata
            },
            has_events: self.has_events || next.has_events,
            event_preview: self.event_preview.clone().or(next.event_preview),
            inherited: next.inherited.or_else(|| self.inherited.clone()),
            importing: None,
        }
    }

    pub(crate) fn record_events(&mut self, events: &[nanocode_durability_core::StoreEvent]) {
        if events.is_empty() {
            return;
        }
        self.has_events = true;
        if self.event_preview.is_none() {
            self.event_preview = events
                .iter()
                .find_map(|event| event.text.as_deref().and_then(preview_text));
        }
    }
}

pub(crate) fn preview_text(text: &str) -> Option<String> {
    (!text.trim().is_empty()).then(|| text.chars().take(500).collect())
}

fn now_ms() -> u64 {
    use nanocode_platform::UNIX_EPOCH;

    nanocode_platform::system_time()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

pub use store::*;

mod store {
    mod transfer;

    use std::{
        collections::{BTreeMap, HashSet},
        path::PathBuf,
        sync::Arc,
    };

    use nanocode_agent_core::{
        HarnessFamily, Lineage, ModelSelection, Origin,
        input::{Prompt, PromptInput, UserInput},
    };
    use serde::Serialize;
    use serde_json::Value;

    use super::{BranchBoundary, SessionRecord};
    use crate::{
        AdmittedTurn, CheckpointFormat, DurableSession, DurableState, EncodedPayload, Error,
        EventHits, EventPage, EventQuery, EventRange, OperationState, OperationStatus, OwnerId,
        PayloadReader, Result, SessionWrite, StateCommit, StateCursor, StateQuery, StateReader,
        StateStore, StateSummary, StoreError, SummaryUpdate, Transition,
        format::Formats,
        session::reduce_peeked,
        shared_store::{SharedReader, SharedStore, WeakSharedStore},
    };

    pub use nanocode_agent_core::session::TranscriptItem;
    pub use transfer::{BranchCursor, BranchExport, BranchImport, BranchPage};

    pub const LIST_LIMIT: usize = 1000;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
    pub struct SessionSummary {
        pub record: SessionRecord,

        pub preview: Option<String>,
    }

    #[derive(Clone, Debug, Default)]
    pub struct SessionQuery {
        pub workspace: Option<PathBuf>,
        pub model: Option<ModelSelection>,
        pub family: Option<HarnessFamily>,
        pub origin: Option<Origin>,
        pub parent_session_id: Option<String>,
        pub text: Vec<String>,
        pub after: Option<StateCursor>,
        pub limit: usize,
    }

    #[derive(Clone, Debug)]
    pub struct SessionPage {
        pub sessions: Vec<SessionSummary>,
        pub next: Option<StateCursor>,
    }

    #[derive(Clone)]
    pub struct SessionReader {
        reader: SharedReader,
    }

    impl SessionReader {
        pub fn new<R: StateReader + 'static>(reader: R) -> Result<Self> {
            Ok(Self {
                reader: SharedReader::new(reader)?,
            })
        }

        pub async fn query(&self, query: &SessionQuery) -> Result<SessionPage> {
            let scope = query
                .workspace
                .as_ref()
                .map(|workspace| {
                    workspace.to_str().map(str::to_owned).ok_or_else(|| {
                        Error::InvalidState(format!(
                            "session query workspace {} is not UTF-8",
                            workspace.display()
                        ))
                    })
                })
                .transpose()?;
            let attributes = [
                ("model", query.model.as_ref().map(ToString::to_string)),
                ("family", query.family.as_ref().map(ToString::to_string)),
                ("origin", query.origin.map(origin_name).transpose()?),
            ]
            .into_iter()
            .filter_map(|(key, value)| Some((key.to_owned(), value?)))
            .collect();
            let page = self
                .reader
                .clone()
                .query_states(&StateQuery {
                    scope,
                    parent: query.parent_session_id.clone(),
                    attributes,
                    text: query.text.clone(),
                    after: query.after.clone(),
                    limit: query.limit,
                })
                .await?;
            let sessions = page
                .states
                .into_iter()
                .map(|state| {
                    let record: SessionRecord = serde_json::from_str(&state.summary.value)?;
                    if record.session_id != state.state_id {
                        return Err(Error::InvalidState(format!(
                            "indexed state {} summarizes session {}",
                            state.state_id, record.session_id
                        )));
                    }
                    Ok(SessionSummary {
                        record,
                        preview: state.summary.preview,
                    })
                })
                .collect::<Result<_>>()?;
            Ok(SessionPage {
                sessions,
                next: page.next,
            })
        }

        pub async fn events(&self, session_id: &str, range: &EventRange) -> Result<EventPage> {
            Ok(self.reader.clone().read_events(session_id, range).await?)
        }

        pub async fn search_events(&self, query: &EventQuery) -> Result<EventHits> {
            Ok(self.reader.clone().query_events(query).await?)
        }

        pub async fn read_records(
            &self,
            session_id: &str,
            keys: &[String],
        ) -> Result<Vec<Option<String>>> {
            Ok(self.reader.clone().read_records(session_id, keys).await?)
        }

        pub async fn state(&self, session_id: &str) -> Result<Option<DurableState>> {
            let mut reader = self.reader.clone();
            let stored = reader.read_state(session_id).await?;
            if stored.payload.is_none() {
                return Ok(None);
            }
            let mut state = reduce_peeked(stored)?;
            state.hydrate_read(&mut reader, session_id).await?;
            Ok(Some(state))
        }

        pub async fn resolve(
            &self,
            session_id: &str,
            payload: &EncodedPayload,
        ) -> Result<EncodedPayload> {
            payload.read(&mut self.reader.clone(), session_id).await
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum TurnStatus {
        Pending,

        Completed,

        Failed,

        Cancelled,
    }

    #[derive(Clone, Debug, PartialEq, Serialize)]
    pub struct StoredTurn {
        pub id: String,

        pub preview: Option<String>,

        pub status: TurnStatus,

        pub input: Value,
    }

    #[derive(Clone, Debug)]
    pub struct StoredSession {
        pub summary: SessionSummary,

        pub transcript: Vec<TranscriptItem>,

        pub turns: Vec<StoredTurn>,

        pub in_progress: Option<String>,

        pub checkpoint: Option<Value>,

        formats: Formats,

        loaded: Loaded,
    }

    #[derive(Clone, Debug)]
    struct Loaded {
        store: WeakSharedStore,
        state_id: String,
        checkpoint_key: Option<String>,
        checkpoint_payload: Option<(EncodedPayload, Arc<HashSet<String>>)>,
        record_inferred: bool,
    }

    impl StoredSession {
        pub fn checkpoint_thinking(&self) -> Result<Option<nanocode_agent_core::Thinking>> {
            let Some(checkpoint) = &self.checkpoint else {
                return Ok(None);
            };
            if is_portable(checkpoint) {
                return Ok(self
                    .session_checkpoint()?
                    .map(|checkpoint| checkpoint.thinking()));
            }
            let record = &self.summary.record;
            self.formats
                .family(record.family())?
                .thinking(record, checkpoint)
                .map(Some)
        }

        pub fn session_checkpoint(&self) -> Result<Option<nanocode_agent_core::SessionCheckpoint>> {
            let Some(checkpoint) = &self.checkpoint else {
                return Ok(None);
            };
            let record = &self.summary.record;
            if is_portable(checkpoint) {
                let portable = portable_checkpoint(checkpoint)?;
                portable
                    .require_family(record.family())
                    .map_err(|error| Error::InvalidState(error.to_string()))?;

                let rebound = nanocode_agent_core::SessionCheckpoint::native(
                    record.session_id.clone(),
                    portable.model(),
                    portable.thinking(),
                    record.lineage.clone(),
                    portable.conversation_id(),
                    portable.turn_id().map(str::to_owned),
                    portable.has_conversation(),
                    portable.payload().clone(),
                )
                .with_service_tier(portable.service_tier());
                rebound
                    .validate()
                    .map_err(|error| Error::InvalidState(error.to_string()))?;
                return Ok(Some(rebound));
            }
            self.formats
                .family(record.family())?
                .session_checkpoint(record, checkpoint.clone())
                .map(Some)
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum BranchPoint {
        Latest,

        Through(String),

        Before(String),
    }

    #[derive(Clone)]
    pub struct SessionStore {
        reader: Option<SessionReader>,
        store: SharedStore,
        formats: Formats,
    }

    pub trait IntoSessionStore {
        fn into_session_store(self) -> Result<SessionStore>;
    }

    impl IntoSessionStore for SessionStore {
        fn into_session_store(self) -> Result<SessionStore> {
            Ok(self)
        }
    }

    impl<S: StateStore + 'static> IntoSessionStore for S {
        fn into_session_store(self) -> Result<SessionStore> {
            SessionStore::new(self)
        }
    }

    impl SessionStore {
        #[must_use]
        pub fn same_store(&self, other: &Self) -> bool {
            self.store.same_store(&other.store)
        }

        pub fn new<S: StateStore + 'static>(store: S) -> Result<Self> {
            Ok(Self {
                reader: None,
                store: SharedStore::new(store)?,
                formats: Formats::default(),
            })
        }

        pub fn register_format(&self, format: Arc<dyn CheckpointFormat>) -> Result<()> {
            self.formats.register(format)
        }

        pub async fn flush(&self) -> Result<()> {
            self.store.clone().flush().await?;
            Ok(())
        }

        pub fn with_reader<R: StateReader + 'static>(mut self, reader: R) -> Result<Self> {
            self.reader = Some(SessionReader::new(reader)?);
            Ok(self)
        }

        #[must_use]
        pub fn reader(&self) -> Option<SessionReader> {
            self.reader.clone()
        }

        pub async fn list(&self) -> Result<Vec<SessionSummary>> {
            let ids = self.store.clone().list_states(usize::MAX).await?;
            let mut sessions: Vec<_> = futures_util::future::try_join_all(
                ids.iter()
                    .filter(|id| !is_auxiliary_state(id))
                    .map(|id| self.summary(id)),
            )
            .await?
            .into_iter()
            .flatten()
            .collect();
            sessions.sort_by_key(|session| std::cmp::Reverse(session.record.updated_at_ms));
            Ok(sessions)
        }

        pub async fn query(&self, query: &SessionQuery) -> Result<SessionPage> {
            self.require_reader()?.query(query).await
        }

        pub async fn events(&self, session_id: &str, range: &EventRange) -> Result<EventPage> {
            self.require_reader()?.events(session_id, range).await
        }

        pub async fn search_events(&self, query: &EventQuery) -> Result<EventHits> {
            self.require_reader()?.search_events(query).await
        }

        pub async fn read_records(
            &self,
            session_id: &str,
            keys: &[String],
        ) -> Result<Vec<Option<String>>> {
            self.require_reader()?.read_records(session_id, keys).await
        }

        fn require_reader(&self) -> Result<&SessionReader> {
            self.reader.as_ref().ok_or_else(|| {
                Error::Store(StoreError::Backend(
                    "this store does not support non-fencing reads".to_owned(),
                ))
            })
        }

        pub async fn summary(&self, id: &str) -> Result<Option<SessionSummary>> {
            let Some(state) = self.peek(id).await? else {
                return Ok(None);
            };
            if !is_listed(&state) {
                return Ok(None);
            }
            let preview = self.first_prompt(id, &state).await?;
            let record = match state.session() {
                Some(record) => record.clone(),
                None => {
                    let Some(checkpoint) = self.native_checkpoint(id, &state).await? else {
                        return Ok(None);
                    };
                    self.infer_record(id, &checkpoint)?
                }
            };
            let preview = preview.or_else(|| record.event_preview.clone());
            Ok(Some(SessionSummary { record, preview }))
        }

        pub async fn record(&self, id: &str) -> Result<Option<SessionRecord>> {
            let Some(state) = self.peek(id).await? else {
                return Ok(None);
            };
            if !is_listed(&state) {
                return Ok(None);
            }
            match state.session() {
                Some(record) => Ok(Some(record.clone())),
                None => match self.native_checkpoint(id, &state).await? {
                    Some(checkpoint) => self.infer_record(id, &checkpoint).map(Some),
                    None => Ok(None),
                },
            }
        }

        pub async fn load(&self, id: &str) -> Result<StoredSession> {
            let state = self.require(id).await?;
            let (checkpoint, checkpoint_payload) = match self.loaded_checkpoint(id, &state).await? {
                Some((checkpoint, payload, records)) => {
                    (Some(checkpoint), Some((payload, Arc::new(records))))
                }
                None => (None, None),
            };
            let record = match (state.session(), &checkpoint) {
                (Some(record), _) => record.clone(),
                (None, Some(checkpoint)) => self.infer_record(id, checkpoint)?,
                (None, None) => {
                    return Err(Error::SessionNotFound {
                        session_id: id.to_owned(),
                    });
                }
            };
            let turns = self.state_turns(id, &state).await?;
            let admitted = admitted_turns(&turns, &state);
            let progress = self.progress(id, &state, record.family()).await?;
            let transcript = match (&progress, &checkpoint) {
                (Some((_, view, steers)), _) => {
                    let mut transcript = self.transcript(view, record.family(), &admitted)?;
                    transcript.extend(steers.iter().cloned().map(TranscriptItem::User));
                    transcript
                }
                (None, Some(checkpoint)) => {
                    self.transcript(checkpoint, record.family(), &admitted)?
                }
                (None, None) => Vec::new(),
            };
            let preview = turns
                .iter()
                .find_map(|turn| turn.preview.clone())
                .or_else(|| {
                    transcript.iter().find_map(|item| match item {
                        TranscriptItem::User(text) => Some(text.clone()),
                        _ => None,
                    })
                });
            Ok(StoredSession {
                summary: SessionSummary { record, preview },
                transcript,
                turns,
                in_progress: progress.map(|(operation, _, _)| operation),
                checkpoint,
                formats: self.formats.clone(),
                loaded: Loaded {
                    store: self.store.downgrade(),
                    state_id: id.to_owned(),
                    checkpoint_key: state
                        .latest_checkpoint()
                        .map(|payload| payload.key().to_owned()),
                    checkpoint_payload,
                    record_inferred: state.session().is_none(),
                },
            })
        }

        pub async fn turns(&self, id: &str) -> Result<Vec<StoredTurn>> {
            let state = self.require(id).await?;
            self.state_turns(id, &state).await
        }

        pub async fn pending_turns(&self, id: &str) -> Result<Vec<StoredTurn>> {
            let state = self.require_head(id).await?;
            self.stored_turns(id, state.pending_operations(), false)
                .await
        }

        pub async fn turn(&self, id: &str, request_id: &str) -> Result<Option<StoredTurn>> {
            let mut state = self.require_head(id).await?;
            if state.operation(request_id).is_none() {
                state
                    .hydrate(&mut self.store.clone(), id, &mut HashSet::new())
                    .await?;
            }
            let Some(operation) = state.operation(request_id) else {
                return Ok(None);
            };
            Ok(self
                .stored_turns(id, vec![(request_id, operation)], false)
                .await?
                .pop())
        }

        pub async fn session(&self, record: SessionRecord) -> Result<DurableSession> {
            let session = DurableSession::open_shared_described(
                self.store.clone(),
                record.session_id.clone(),
                None,
                Some(record),
            )
            .await?;
            self.publish_opened(session).await
        }

        async fn publish_opened(&self, session: DurableSession) -> Result<DurableSession> {
            if self.reader.is_some() && self.store.supports_summaries() {
                let state = session.state().await?;
                if state.session().is_some() && is_listed(&state) {
                    session.append(SessionWrite::default()).await?;
                }
            }
            Ok(session)
        }

        pub async fn prepare_session(&self, record: SessionRecord) -> Result<DurableSession> {
            DurableSession::prepare_new(self.store.clone(), record)
        }

        pub async fn recovery_checkpoint(
            &self,
            journal: nanocode_agent_core::SessionCheckpoint,
        ) -> Result<nanocode_agent_core::SessionCheckpoint> {
            journal
                .validate()
                .map_err(|error| Error::InvalidState(error.to_string()))?;
            let stored = match self.load(journal.session_id()).await {
                Ok(stored) => stored,
                Err(Error::SessionNotFound { .. }) => return Ok(journal),
                Err(error) => return Err(error),
            };
            let record = &stored.summary.record;
            if record.family() != journal.family() || record.lineage != *journal.lineage() {
                return Err(Error::InvalidState(
                    "child journal identity differs from its durable session".into(),
                ));
            }
            Ok(stored.session_checkpoint()?.unwrap_or(journal))
        }

        pub async fn resume(&self, id: &str) -> Result<DurableSession> {
            let record = self.resume_record(id, None).await?;
            let session = DurableSession::open_shared_described(
                self.store.clone(),
                id.to_owned(),
                None,
                record,
            )
            .await?;
            self.publish_opened(session).await
        }

        pub async fn prepare_resume(&self, id: &str) -> Result<DurableSession> {
            self.prepare_resume_with(id, None).await
        }

        async fn prepare_resume_with(
            &self,
            id: &str,
            loaded: Option<(&str, &Value)>,
        ) -> Result<DurableSession> {
            let record = self.resume_record(id, loaded).await?;
            DurableSession::prepare_shared_described(
                self.store.clone(),
                id.to_owned(),
                None,
                record,
            )
            .await
        }

        pub async fn prepare_resume_loaded(
            &self,
            stored: &StoredSession,
        ) -> Result<DurableSession> {
            if !stored.loaded.store.is(&self.store) {
                return Err(Error::InvalidState(
                    "the stored session was loaded from a different session store".to_owned(),
                ));
            }
            let id = &stored.summary.record.session_id;
            if *id != stored.loaded.state_id {
                return Err(Error::InvalidState(
                    "the stored session identity changed after loading".to_owned(),
                ));
            }
            let state = if stored.loaded.record_inferred {
                let inferred = match (&stored.loaded.checkpoint_key, &stored.checkpoint) {
                    (Some(key), Some(checkpoint)) => Some((key.as_str(), checkpoint)),
                    _ => None,
                };
                self.prepare_resume_with(id, inferred).await?
            } else {
                DurableSession::prepare_shared_described(self.store.clone(), id.clone(), None, None)
                    .await?
            };
            if let (Some(key), Some(checkpoint)) =
                (&stored.loaded.checkpoint_key, &stored.checkpoint)
            {
                state.preload_checkpoint(
                    key.clone(),
                    checkpoint.clone(),
                    stored.loaded.checkpoint_payload.clone(),
                );
            }
            Ok(state)
        }

        async fn resume_record(
            &self,
            id: &str,
            loaded: Option<(&str, &Value)>,
        ) -> Result<Option<SessionRecord>> {
            let state = self.require_head(id).await?;
            if state.session().is_some() {
                return Ok(None);
            }
            if let (Some(payload), Some((key, checkpoint))) = (state.latest_checkpoint(), loaded)
                && payload.key() == key
            {
                return self.infer_record(id, checkpoint).map(Some);
            }
            self.native_checkpoint(id, &state)
                .await?
                .map(|checkpoint| self.infer_record(id, &checkpoint))
                .transpose()
        }

        pub async fn branch(
            &self,
            id: &str,
            at: BranchPoint,
            workspace: Option<PathBuf>,
        ) -> Result<SessionSummary> {
            self.branch_with(id, at, workspace, |selected, _| Ok(selected))
                .await
        }

        pub async fn branch_with(
            &self,
            id: &str,
            at: BranchPoint,
            workspace: Option<PathBuf>,
            transform: impl FnOnce(Option<Value>, Option<&Value>) -> Result<Option<Value>>,
        ) -> Result<SessionSummary> {
            let state = self.require(id).await?;

            if !state.pending_operations().is_empty() {
                return Err(Error::InvalidState(
                    "conversation rewind refuses pending operations; settle or reconcile them first"
                        .into(),
                ));
            }
            let mut record = self.source_record(id, &state).await?;
            let (selected, boundary) = match self.inherited_point(id, &state, &at).await? {
                Some((selected, boundary)) => {
                    (selected.map(|payload| (id.to_owned(), payload)), boundary)
                }
                None => (
                    self.local_point(id, &state, &at)?,
                    self.boundary(id, &state, &at).await?,
                ),
            };
            let branch_id = nanocode_platform::uuid_v7().to_string();
            let selected = match selected {
                Some((holder, payload)) => Some(
                    self.hydrate(&holder, &payload, Some(record.family()))
                        .await?,
                ),
                None => None,
            };
            let latest = self.native_checkpoint(id, &state).await?;
            let mut native = transform(selected, latest.as_ref())?;
            if let (Some(workspace), Some(Value::Object(object))) = (&workspace, native.as_mut())
                && object.contains_key("workspace")
            {
                object.insert(
                    "workspace".into(),
                    Value::String(workspace.display().to_string()),
                );
            }
            record = record.derive(branch_id, Origin::Branch);
            record.branch = Some(boundary);
            if workspace.is_some() {
                record.workspace = workspace;
            }
            let checkpoint = native
                .map(|value| self.encode_native(record.family(), value))
                .transpose()?;

            record.start_checkpoint = checkpoint.as_ref().map(|payload| payload.key().to_owned());
            let preview = match &at {
                BranchPoint::Before(_) => None,
                _ => self.first_prompt(id, &state).await?,
            };
            self.publish(record.clone(), checkpoint, preview.clone())
                .await?;
            Ok(SessionSummary { record, preview })
        }

        async fn source_record(&self, id: &str, state: &DurableState) -> Result<SessionRecord> {
            if let Some(record) = state.session() {
                return Ok(record.clone());
            }
            let checkpoint = self.native_checkpoint(id, state).await?.ok_or_else(|| {
                Error::InvalidState("source session has no checkpoint to branch from".into())
            })?;
            self.infer_record(id, &checkpoint)
        }

        fn local_point(
            &self,
            id: &str,
            state: &DurableState,
            at: &BranchPoint,
        ) -> Result<Option<(String, EncodedPayload)>> {
            Ok(match at {
                BranchPoint::Latest => state
                    .latest_checkpoint()
                    .cloned()
                    .map(|payload| (id.to_owned(), payload)),
                BranchPoint::Through(turn) => {
                    let operation = state.operation(turn).ok_or_else(unknown_turn)?;
                    Some(
                        settled_checkpoint(&operation.status)
                            .cloned()
                            .map(|payload| (id.to_owned(), payload))
                            .ok_or_else(|| {
                                Error::InvalidState(
                                    "the selected turn has no settled checkpoint".into(),
                                )
                            })?,
                    )
                }
                BranchPoint::Before(turn) => {
                    let selected = state.operation(turn).ok_or_else(unknown_turn)?;
                    let prior = state
                        .operations()
                        .values()
                        .filter(|op| op.accepted_order() < selected.accepted_order())
                        .max_by_key(|op| op.accepted_order());

                    let complete_history = state
                        .session()
                        .map_or(selected.accepted_order() == 1, |record| {
                            !record.history_pruned
                        });
                    match prior {
                        None if complete_history => Self::start_checkpoint(id, state)?,
                        Some(prior) => Some(
                            settled_checkpoint(&prior.status)
                                .cloned()
                                .map(|payload| (id.to_owned(), payload))
                                .ok_or_else(|| {
                                    Error::InvalidState(
                                        "checkpoint before the selected turn is unavailable".into(),
                                    )
                                })?,
                        ),
                        None => {
                            return Err(Error::InvalidState(
                                "retained history before the selected turn was pruned".into(),
                            ));
                        }
                    }
                }
            })
        }

        fn start_checkpoint(
            id: &str,
            state: &DurableState,
        ) -> Result<Option<(String, EncodedPayload)>> {
            let Some(record) = state.session() else {
                return Ok(None);
            };
            if matches!(record.lineage.origin, Origin::Root | Origin::Subagent) {
                return Ok(None);
            }
            match (&record.start_checkpoint, &record.branch) {
                (Some(key), _) => Ok(Some((id.to_owned(), EncodedPayload::from_key(key)))),
                (None, Some(_)) => Ok(None),
                (None, None) => Err(Error::InvalidState(
                    "history before this session's first turn is held by its source session, which this older session record does not pin".into(),
                )),
            }
        }

        async fn boundary(
            &self,
            id: &str,
            state: &DurableState,
            at: &BranchPoint,
        ) -> Result<BranchBoundary> {
            let order = |turn: &str| state.operation(turn).map(|op| op.accepted_order());
            let limit = match at {
                BranchPoint::Latest => state
                    .operations()
                    .values()
                    .filter(|op| settled_checkpoint(&op.status).is_some())
                    .map(|op| op.accepted_order())
                    .max()
                    .unwrap_or(0),
                BranchPoint::Through(turn) => order(turn).ok_or_else(unknown_turn)?,
                BranchPoint::Before(turn) => {
                    order(turn).ok_or_else(unknown_turn)?.saturating_sub(1)
                }
            };
            let through_turn = self
                .state_turns(id, state)
                .await?
                .into_iter()
                .rev()
                .find(|turn| order(&turn.id).is_some_and(|order| order <= limit))
                .map(|turn| turn.id);
            Ok(BranchBoundary {
                source_session_id: id.to_owned(),
                through_turn,
            })
        }

        async fn publish(
            &self,
            record: SessionRecord,
            checkpoint: Option<EncodedPayload>,
            preview: Option<String>,
        ) -> Result<()> {
            let mut store = self.store.clone();
            let id = record.session_id.clone();
            let target = store.acquire(&id, OwnerId::new()).await?;
            if target.state.revision != 0 || target.state.payload.is_some() {
                return Err(Error::InvalidState(
                    "new branch identity unexpectedly exists".into(),
                ));
            }
            let mut state = DurableState::default();
            match checkpoint {
                Some(checkpoint) => {
                    state.apply_transition(1, Transition::CheckpointCommitted { checkpoint })?;
                }
                None => state.advance_revision(1)?,
            }
            state.set_session(Some(record));
            let records = state.stage_records();
            let summary = if store.supports_summaries() {
                match summarize(&state, preview)? {
                    Some(summary) => SummaryUpdate::Set(summary),
                    None => SummaryUpdate::Unchanged,
                }
            } else {
                SummaryUpdate::Unchanged
            };
            store
                .commit(
                    &id,
                    &target.owner,
                    0,
                    StateCommit {
                        payload: &state.checkpoint_payload()?,
                        records: &records,
                        events: &[],
                        summary: &summary,
                    },
                )
                .await?;
            Ok(())
        }

        async fn peek(&self, id: &str) -> Result<Option<DurableState>> {
            let Some(mut state) = self.peek_head(id).await? else {
                return Ok(None);
            };
            state
                .hydrate(&mut self.store.clone(), id, &mut HashSet::new())
                .await?;
            Ok(Some(state))
        }

        async fn peek_head(&self, id: &str) -> Result<Option<DurableState>> {
            let stored = self.store.clone().peek(id).await?;
            if stored.payload.is_none() {
                return Ok(None);
            }
            let state = reduce_peeked(stored)?;
            if state
                .session()
                .is_some_and(|record| record.importing.is_some())
            {
                return Ok(None);
            }
            Ok(Some(state))
        }

        async fn require(&self, id: &str) -> Result<DurableState> {
            self.peek(id).await?.ok_or_else(|| Error::SessionNotFound {
                session_id: id.to_owned(),
            })
        }

        async fn require_head(&self, id: &str) -> Result<DurableState> {
            self.peek_head(id)
                .await?
                .ok_or_else(|| Error::SessionNotFound {
                    session_id: id.to_owned(),
                })
        }

        async fn load_payload(&self, id: &str, payload: &EncodedPayload) -> Result<EncodedPayload> {
            payload.load(&mut self.store.clone(), id).await
        }

        async fn hydrate(
            &self,
            id: &str,
            payload: &EncodedPayload,
            family: Option<HarnessFamily>,
        ) -> Result<Value> {
            let value: Value = self.load_payload(id, payload).await?.decode()?;
            self.hydrate_value(id, value, family, &mut HashSet::new())
                .await
        }

        async fn hydrate_value(
            &self,
            id: &str,
            value: Value,
            family: Option<HarnessFamily>,
            read: &mut HashSet<String>,
        ) -> Result<Value> {
            if is_portable(&value) {
                return Ok(value);
            }
            let format = match family {
                Some(family) => self.formats.family(family)?,
                None => self.formats.owner(&value)?,
            };
            format
                .hydrate_noting(PayloadReader::store(&self.store, id), value, read)
                .await
        }

        fn encode_native(&self, family: HarnessFamily, value: Value) -> Result<EncodedPayload> {
            if is_portable(&value) {
                portable_checkpoint(&value)?
                    .require_family(family)
                    .map_err(|error| Error::InvalidState(error.to_string()))?;
                return EncodedPayload::encode(&value);
            }
            self.formats.family(family)?.encode(value)
        }

        fn infer_record(&self, id: &str, checkpoint: &Value) -> Result<SessionRecord> {
            if is_portable(checkpoint) {
                let checkpoint = portable_checkpoint(checkpoint)?;
                let mut record = SessionRecord::root(id, checkpoint.model(), None)
                    .with_lineage(checkpoint.lineage().clone());
                record.created_at_ms = 0;
                record.updated_at_ms = 0;
                return Ok(record);
            }
            let model = checkpoint
                .get("model")
                .and_then(Value::as_str)
                .and_then(|model| model.parse::<ModelSelection>().ok())
                .ok_or_else(|| {
                    Error::InvalidState("stored checkpoint has no known model".into())
                })?;
            if !self.formats.family(model.family())?.owns(checkpoint) {
                return Err(Error::InvalidState(
                    "stored checkpoint model belongs to another family".into(),
                ));
            }
            let workspace = checkpoint
                .get("workspace")
                .and_then(Value::as_str)
                .filter(|workspace| !workspace.is_empty())
                .map(PathBuf::from);
            Ok(SessionRecord {
                session_id: id.to_owned(),
                model,
                workspace,
                title: None,
                lineage: Lineage::root(id),
                created_at_ms: 0,
                updated_at_ms: 0,
                history_pruned: false,
                branch: None,
                start_checkpoint: None,
                initial: None,
                inherited_transcript: Vec::new(),
                metadata: BTreeMap::new(),
                has_events: false,
                event_preview: None,
                inherited: None,
                importing: None,
            })
        }

        fn transcript(
            &self,
            checkpoint: &Value,
            family: HarnessFamily,
            turns: &[AdmittedTurn<'_>],
        ) -> Result<Vec<TranscriptItem>> {
            let checkpoint = if is_portable(checkpoint) {
                &checkpoint["payload"]
            } else {
                checkpoint
            };
            Ok(self.formats.family(family)?.transcript(checkpoint, turns))
        }

        async fn progress(
            &self,
            id: &str,
            state: &DurableState,
            family: HarnessFamily,
        ) -> Result<Option<(String, Value, Vec<String>)>> {
            let Some((operation_id, operation)) = state.first_pending_operation() else {
                return Ok(None);
            };
            let Some(continuation) = &operation.continuation else {
                return Ok(None);
            };
            let continuation: Value = self.load_payload(id, continuation).await?.decode()?;
            let Some(view) = self
                .formats
                .family(family)?
                .hydrate_continuation(PayloadReader::store(&self.store, id), continuation)
                .await?
            else {
                return Ok(None);
            };
            let unbound: Vec<_> = operation
                .steers
                .iter()
                .filter(|steer| steer.model_call_index.is_none())
                .map(|steer| steer.input.clone())
                .collect();
            let mut steers = Vec::new();
            for input in EncodedPayload::load_many(&unbound, &mut self.store.clone(), id).await? {
                let prompt: Prompt = input.decode()?;
                steers.extend(instruction_text(&prompt.instruction));
            }
            Ok(Some((operation_id.to_owned(), view, steers)))
        }

        async fn loaded_checkpoint(
            &self,
            id: &str,
            state: &DurableState,
        ) -> Result<Option<(Value, EncodedPayload, HashSet<String>)>> {
            let Some(payload) = state.latest_checkpoint() else {
                return Ok(None);
            };
            let mut records = HashSet::new();
            let loaded = payload
                .load_noting(&mut self.store.clone(), id, &mut records)
                .await?;
            let value = self
                .hydrate_value(
                    id,
                    loaded.decode()?,
                    state.session().map(SessionRecord::family),
                    &mut records,
                )
                .await?;
            Ok(Some((value, loaded, records)))
        }

        async fn native_checkpoint(&self, id: &str, state: &DurableState) -> Result<Option<Value>> {
            match state.latest_checkpoint() {
                Some(payload) => self
                    .hydrate(id, payload, state.session().map(SessionRecord::family))
                    .await
                    .map(Some),
                None => Ok(None),
            }
        }

        async fn state_turns(&self, id: &str, state: &DurableState) -> Result<Vec<StoredTurn>> {
            let mut operations: Vec<_> = state
                .operations()
                .iter()
                .map(|(turn, op)| (turn.as_str(), op))
                .collect();
            operations.sort_by_key(|(_, op)| op.accepted_order());
            self.stored_turns(id, operations, true).await
        }

        async fn stored_turns(
            &self,
            id: &str,
            operations: Vec<(&str, &OperationState)>,
            prompts_only: bool,
        ) -> Result<Vec<StoredTurn>> {
            let inputs: Vec<_> = operations.iter().map(|(_, op)| op.input.clone()).collect();
            let inputs = EncodedPayload::load_many(&inputs, &mut self.store.clone(), id).await?;
            let mut turns = Vec::new();
            for ((turn, operation), input) in operations.into_iter().zip(inputs) {
                let input: Value = input.decode()?;
                if prompts_only
                    && input
                        .get("kind")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind != "prompt")
                {
                    continue;
                }
                turns.push(StoredTurn {
                    id: turn.to_owned(),
                    preview: prompt_text(&input),
                    status: match operation.status {
                        OperationStatus::Pending => TurnStatus::Pending,
                        OperationStatus::Completed { .. } => TurnStatus::Completed,
                        OperationStatus::Failed { .. } => TurnStatus::Failed,
                        OperationStatus::Cancelled { .. } => TurnStatus::Cancelled,
                    },
                    input,
                });
            }
            Ok(turns)
        }

        async fn first_prompt(&self, id: &str, state: &DurableState) -> Result<Option<String>> {
            let Some((_, first)) = state
                .operations()
                .iter()
                .min_by_key(|(_, op)| op.accepted_order())
            else {
                return Ok(None);
            };
            let input: Value = self.load_payload(id, &first.input).await?.decode()?;
            Ok(prompt_text(&input))
        }
    }

    fn unknown_turn() -> Error {
        Error::InvalidState("unknown or expired turn".into())
    }

    const fn settled_checkpoint(status: &OperationStatus) -> Option<&EncodedPayload> {
        match status {
            OperationStatus::Completed { checkpoint, .. }
            | OperationStatus::Failed { checkpoint, .. }
            | OperationStatus::Cancelled {
                checkpoint: Some(checkpoint),
            } => Some(checkpoint),
            _ => None,
        }
    }

    fn is_auxiliary_state(id: &str) -> bool {
        id.ends_with(":subagents") || id.ends_with(":process-routes")
    }

    pub(crate) fn is_listed(state: &DurableState) -> bool {
        let created_child = state.session().is_some_and(|record| {
            matches!(
                record.lineage.origin,
                Origin::Fork | Origin::SideConversation | Origin::Subagent
            )
        });
        let has_events = state.session().is_some_and(|record| record.has_events);
        state.has_operations() || state.latest_checkpoint().is_some() || created_child || has_events
    }

    pub(crate) fn summarize(
        state: &DurableState,
        preview: Option<String>,
    ) -> Result<Option<StateSummary>> {
        let Some(record) = state.session().filter(|_| is_listed(state)) else {
            return Ok(None);
        };
        let attributes = BTreeMap::from([
            ("model".to_owned(), record.model.to_string()),
            ("family".to_owned(), record.family().to_string()),
            ("origin".to_owned(), origin_name(record.lineage.origin)?),
        ]);
        Ok(Some(StateSummary {
            scope: record
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.to_str())
                .map(str::to_owned),
            parent: record.lineage.parent_session_id.clone(),
            title: record.title.clone(),
            preview: preview.or_else(|| record.event_preview.clone()),
            attributes,
            updated_at_ms: record.updated_at_ms,
            value: serde_json::to_string(record)?,
        }))
    }

    fn origin_name(origin: Origin) -> Result<String> {
        match serde_json::to_value(origin)? {
            Value::String(name) => Ok(name),
            other => Err(Error::InvalidState(format!(
                "session origin serialized as {other} instead of a string"
            ))),
        }
    }

    fn is_portable(value: &Value) -> bool {
        value
            .get("format")
            .and_then(Value::as_str)
            .is_some_and(|format| format.starts_with("nanocode-session-checkpoint/"))
    }

    fn portable_checkpoint(value: &Value) -> Result<nanocode_agent_core::SessionCheckpoint> {
        let checkpoint: nanocode_agent_core::SessionCheckpoint =
            serde_json::from_value(value.clone()).map_err(Error::InvalidPayload)?;
        checkpoint
            .validate()
            .map_err(|error| Error::InvalidState(error.to_string()))?;
        Ok(checkpoint)
    }

    pub(crate) fn prompt_text(input: &Value) -> Option<String> {
        fn find(value: &Value, depth: usize) -> Option<&str> {
            if depth > 8 {
                return None;
            }
            match value {
                Value::Object(object) => {
                    for key in ["instruction", "text", "prompt", "message"] {
                        if let Some(text) = object.get(key).and_then(Value::as_str)
                            && !text.trim().is_empty()
                        {
                            return Some(text);
                        }
                    }
                    object.values().find_map(|value| find(value, depth + 1))
                }
                Value::Array(items) => items.iter().find_map(|value| find(value, depth + 1)),
                _ => None,
            }
        }
        find(input, 0).and_then(super::preview_text)
    }

    fn instruction_text(instruction: &PromptInput) -> Option<String> {
        let text = match instruction {
            PromptInput::Text(text) => text.clone(),
            PromptInput::Content(items) => items
                .iter()
                .filter_map(|item| match item {
                    UserInput::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };
        (!text.trim().is_empty()).then_some(text)
    }

    fn admitted_turns<'a>(turns: &'a [StoredTurn], state: &DurableState) -> Vec<AdmittedTurn<'a>> {
        turns
            .iter()
            .map(|turn| AdmittedTurn {
                input: &turn.input,
                steers: state.operation(&turn.id).map_or(0, |operation| {
                    operation.retired_steers as usize
                        + operation
                            .steers
                            .iter()
                            .filter(|steer| steer.model_call_index.is_some())
                            .count()
                }),
            })
            .collect()
    }
}
