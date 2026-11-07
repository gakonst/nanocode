use nanocode_durability::{OwnedState, OwnerId, SharedStore, StateStore, StoreRecord, StoredState};
use nanocode_sync::{Atom, Owned};
use nanocode_tool_host::code_mode::{CellJournal, CellJournalSink, CellJournalUpdate};
use nanocode_tool_host::embedded::HostFuture;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

pub(crate) struct DurableCellJournals {
    store: SharedStore,
    sessions: Atom<HashMap<String, Arc<Owned<Session>>>>,
}

#[derive(Default)]
enum Session {
    #[default]
    Unread,
    Owned {
        owned: OwnedState,
        cells: BTreeMap<u64, BTreeSet<u64>>,
    },
}

impl DurableCellJournals {
    pub(crate) fn new(store: SharedStore) -> Arc<Self> {
        Arc::new(Self {
            store,
            sessions: Atom::default(),
        })
    }

    fn session(&self, session_id: &str) -> Arc<Owned<Session>> {
        if let Some(session) = self
            .sessions
            .read(|sessions| sessions.get(session_id).cloned())
        {
            return session;
        }
        self.sessions.update(|sessions| {
            Arc::clone(
                sessions
                    .entry(session_id.to_owned())
                    .or_insert_with(|| Arc::new(Owned::new(Session::Unread))),
            )
        })
    }
}

fn state_id(session_id: &str) -> String {
    format!("{session_id}:cells")
}

fn key(cell_id: u64, seq: u64) -> String {
    format!("{cell_id}/{seq}")
}

async fn owned<'a>(
    store: &SharedStore,
    session_id: &str,
    session: &'a mut Session,
) -> Result<(&'a mut OwnedState, &'a mut BTreeMap<u64, BTreeSet<u64>>), String> {
    if matches!(session, Session::Unread) {
        let owned = store
            .clone()
            .acquire(&state_id(session_id), OwnerId::new())
            .await
            .map_err(|error| error.to_string())?;
        let cells = owned
            .state
            .payload
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        *session = Session::Owned { owned, cells };
    }
    match session {
        Session::Owned { owned, cells } => Ok((owned, cells)),
        Session::Unread => Err("cell journals were not read".to_owned()),
    }
}

async fn replace(
    store: &SharedStore,
    session_id: &str,
    owned: &mut OwnedState,
    cells: &BTreeMap<u64, BTreeSet<u64>>,
    records: &[StoreRecord],
) -> Result<(), String> {
    let payload = serde_json::to_string(cells).map_err(|error| error.to_string())?;
    let revision = store
        .clone()
        .replace(
            &state_id(session_id),
            &owned.owner,
            owned.state.revision,
            &payload,
            records,
        )
        .await
        .map_err(|error| error.to_string())?;
    owned.state = StoredState {
        revision,
        payload: Some(payload),
    };
    Ok(())
}

fn blanked(cell_id: u64, seqs: BTreeSet<u64>) -> impl Iterator<Item = StoreRecord> {
    seqs.into_iter().map(move |seq| StoreRecord {
        key: key(cell_id, seq),
        value: String::new(),
    })
}

impl CellJournalSink for DurableCellJournals {
    fn save(&self, update: CellJournalUpdate) -> HostFuture<'static, Result<(), String>> {
        let store = self.store.clone();
        let session = self.session(&update.session_id);
        Box::pin(async move {
            let mut session = session.acquire().await;
            let (owned, cells) = owned(&store, &update.session_id, &mut session).await?;
            let mut records = Vec::new();
            if update.header.is_some()
                && let Some(seqs) = cells.remove(&update.cell_id)
            {
                records.extend(blanked(update.cell_id, seqs));
            }
            records.push(StoreRecord {
                key: key(update.cell_id, update.seq),
                value: serde_json::to_string(&update).map_err(|error| error.to_string())?,
            });
            cells.entry(update.cell_id).or_default().insert(update.seq);
            replace(&store, &update.session_id, owned, cells, &records).await
        })
    }

    fn forget(&self, session_id: &str, cell_id: u64) -> HostFuture<'static, ()> {
        let store = self.store.clone();
        let session = self.session(session_id);
        let session_id = session_id.to_owned();
        Box::pin(async move {
            let mut session = session.acquire().await;
            let forgotten = async {
                let (owned, cells) = owned(&store, &session_id, &mut session).await?;
                let Some(seqs) = cells.remove(&cell_id) else {
                    return Ok(());
                };
                let records = blanked(cell_id, seqs).collect::<Vec<_>>();
                replace(&store, &session_id, owned, cells, &records).await
            };
            if let Err(error) = forgotten.await {
                tracing::warn!(cell_id, %error, "a Code Mode cell journal could not be forgotten");
            }
        })
    }

    fn load(&self, session_id: &str) -> HostFuture<'static, Result<Vec<CellJournal>, String>> {
        let store = self.store.clone();
        let session = self.session(session_id);
        let session_id = session_id.to_owned();
        Box::pin(async move {
            let mut session = session.acquire().await;
            let (_, cells) = owned(&store, &session_id, &mut session).await?;
            let keys = cells
                .iter()
                .flat_map(|(cell_id, seqs)| seqs.iter().map(|seq| key(*cell_id, *seq)))
                .collect::<Vec<_>>();
            if keys.is_empty() {
                return Ok(Vec::new());
            }
            let values = store
                .clone()
                .read_records(&state_id(&session_id), &keys)
                .await
                .map_err(|error| error.to_string())?;
            let mut updates = BTreeMap::<u64, Vec<CellJournalUpdate>>::new();
            for value in values
                .into_iter()
                .flatten()
                .filter(|value| !value.is_empty())
            {
                let update = serde_json::from_str::<CellJournalUpdate>(&value)
                    .map_err(|error| error.to_string())?;
                updates.entry(update.cell_id).or_default().push(update);
            }
            Ok(updates
                .into_values()
                .filter_map(CellJournal::fold)
                .collect())
        })
    }
}
