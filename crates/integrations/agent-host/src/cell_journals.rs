use nanocode_durability::{
    EncodedPayload, OwnedState, OwnerId, Settle, SharedStore, StateCommit, StateStore, StoreRecord,
    StoredState, SummaryUpdate,
};
use nanocode_sync::{Atom, Owned};
use nanocode_tool_host::code_mode::{
    CellJournal, CellJournalSink, CellJournalUpdate, JournaledCallState,
};
use nanocode_tool_host::embedded::HostFuture;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::{Arc, Weak},
};

const VERSION: u32 = 2;

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
        cells: BTreeMap<u64, Cell>,
        catalogs: HashSet<String>,
    },
}

enum Cell {
    Numbered(BTreeSet<u64>),
    Chained(Vec<String>),
}

#[derive(serde::Deserialize)]
struct Head {
    version: u32,
    #[serde(default)]
    cells: BTreeMap<u64, String>,
    #[serde(default)]
    numbered: BTreeMap<u64, BTreeSet<u64>>,
}

#[derive(serde::Deserialize)]
struct Versioned {
    version: Option<serde::de::IgnoredAny>,
}

#[derive(serde::Serialize)]
struct HeadRef<'a> {
    version: u32,
    cells: BTreeMap<u64, &'a str>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    numbered: BTreeMap<u64, &'a BTreeSet<u64>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Link {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    catalog: Option<String>,
    update: CellJournalUpdate,
}

impl DurableCellJournals {
    pub(crate) fn new(store: SharedStore) -> Arc<Self> {
        Arc::new(Self {
            store,
            sessions: Atom::default(),
        })
    }

    fn catalog(tools: &Arc<[Value]>) -> Result<EncodedPayload, String> {
        type Catalogs = Vec<(Weak<[Value]>, EncodedPayload)>;
        static CATALOGS: std::sync::LazyLock<Atom<Catalogs>> =
            std::sync::LazyLock::new(Atom::default);
        let known = CATALOGS.read(|catalogs| {
            catalogs
                .iter()
                .find(|(held, _)| std::ptr::eq(held.as_ptr(), Arc::as_ptr(tools)))
                .map(|(_, catalog)| catalog.clone())
        });
        if let Some(catalog) = known {
            return Ok(catalog);
        }
        let catalog = EncodedPayload::encode(&**tools).map_err(|error| error.to_string())?;
        CATALOGS.update(|catalogs| {
            catalogs.retain(|(held, _)| held.strong_count() > 0);
            catalogs.push((Arc::downgrade(tools), catalog.clone()));
        });
        Ok(catalog)
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

async fn chain(store: &SharedStore, state: &str, tip: String) -> Result<Vec<String>, String> {
    let mut keys = vec![tip];
    while let Some(key) = keys.last() {
        let link: Link = EncodedPayload::by_key(key)
            .load(&mut store.clone(), state)
            .await
            .and_then(|payload| payload.decode())
            .map_err(|error| error.to_string())?;
        match link.previous {
            Some(previous) => keys.push(previous),
            None => break,
        }
    }
    keys.reverse();
    Ok(keys)
}

async fn owned<'a>(
    store: &SharedStore,
    session_id: &str,
    session: &'a mut Session,
) -> Result<
    (
        &'a mut OwnedState,
        &'a mut BTreeMap<u64, Cell>,
        &'a mut HashSet<String>,
    ),
    String,
> {
    if matches!(session, Session::Unread) {
        let state = state_id(session_id);
        let owned = store
            .clone()
            .acquire(&state, OwnerId::new())
            .await
            .map_err(|error| error.to_string())?;
        let head = match owned.state.payload.as_deref() {
            None => Head {
                version: VERSION,
                cells: BTreeMap::new(),
                numbered: BTreeMap::new(),
            },
            Some(payload) => {
                let versioned = serde_json::from_str::<Versioned>(payload)
                    .map_err(|error| error.to_string())?
                    .version
                    .is_some();
                if versioned {
                    serde_json::from_str(payload).map_err(|error| error.to_string())?
                } else {
                    Head {
                        version: VERSION,
                        cells: BTreeMap::new(),
                        numbered: serde_json::from_str(payload)
                            .map_err(|error| error.to_string())?,
                    }
                }
            }
        };
        if head.version != VERSION {
            return Err(format!(
                "cell journals of session {session_id} have format {}, and this release reads {VERSION}",
                head.version
            ));
        }
        let mut cells = head
            .numbered
            .into_iter()
            .map(|(cell, seqs)| (cell, Cell::Numbered(seqs)))
            .collect::<BTreeMap<_, _>>();
        for (cell, tip) in head.cells {
            cells.insert(cell, Cell::Chained(chain(store, &state, tip).await?));
        }
        *session = Session::Owned {
            owned,
            cells,
            catalogs: HashSet::new(),
        };
    }
    match session {
        Session::Owned {
            owned,
            cells,
            catalogs,
        } => Ok((owned, cells, catalogs)),
        Session::Unread => Err("cell journals were not read".to_owned()),
    }
}

async fn replace(
    store: &SharedStore,
    session_id: &str,
    owned: &mut OwnedState,
    cells: &BTreeMap<u64, Cell>,
    records: &[StoreRecord],
    settle: Settle,
) -> Result<(), String> {
    let mut head = HeadRef {
        version: VERSION,
        cells: BTreeMap::new(),
        numbered: BTreeMap::new(),
    };
    for (cell, journal) in cells {
        match journal {
            Cell::Numbered(seqs) => {
                head.numbered.insert(*cell, seqs);
            }
            Cell::Chained(keys) => {
                if let Some(tip) = keys.last() {
                    head.cells.insert(*cell, tip);
                }
            }
        }
    }
    let payload = serde_json::to_string(&head).map_err(|error| error.to_string())?;
    let committed = store
        .clone()
        .commit(
            &state_id(session_id),
            &owned.owner,
            owned.state.revision,
            StateCommit {
                payload: &payload,
                records,
                events: &[],
                summary: &SummaryUpdate::Unchanged,
                settle,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    owned.state = StoredState {
        revision: committed.revision,
        payload: Some(payload),
    };
    Ok(())
}

fn blanked(cell_id: u64, cell: Cell) -> Vec<StoreRecord> {
    let keys = match cell {
        Cell::Numbered(seqs) => seqs.into_iter().map(|seq| key(cell_id, seq)).collect(),
        Cell::Chained(keys) => keys,
    };
    keys.into_iter()
        .map(|key| StoreRecord {
            key,
            value: String::new(),
        })
        .collect()
}

impl CellJournalSink for DurableCellJournals {
    fn save(&self, mut update: CellJournalUpdate) -> HostFuture<'static, Result<(), String>> {
        let store = self.store.clone();
        let session = self.session(&update.session_id);
        let catalog = update
            .header
            .as_mut()
            .map(|header| Self::catalog(&std::mem::take(&mut header.tools)));
        let settle = if update
            .calls
            .iter()
            .all(|call| matches!(call.state, JournaledCallState::Completed { .. }))
        {
            Settle::Ordered
        } else {
            Settle::Durable
        };
        Box::pin(async move {
            let mut session = session.acquire().await;
            let (owned, cells, catalogs) = owned(&store, &update.session_id, &mut session).await?;
            let mut records = Vec::new();
            let (mut keys, catalog) = match catalog {
                Some(catalog) => {
                    if let Some(replaced) = cells.remove(&update.cell_id) {
                        records.extend(blanked(update.cell_id, replaced));
                    }
                    let mut catalog = catalog?;
                    if catalogs.insert(catalog.key().to_owned()) {
                        let mut pooled = Vec::new();
                        catalog.stage(&mut pooled);
                        store
                            .pool(&state_id(&update.session_id), pooled)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    (Vec::new(), Some(catalog.key().to_owned()))
                }
                None => match cells.remove(&update.cell_id) {
                    Some(Cell::Chained(keys)) => (keys, None),
                    Some(Cell::Numbered(_)) | None => {
                        return Err(format!(
                            "cell {} has no journal to continue",
                            update.cell_id
                        ));
                    }
                },
            };
            let (session_id, cell_id) = (update.session_id.clone(), update.cell_id);
            let mut link = EncodedPayload::encode(&Link {
                previous: keys.last().cloned(),
                catalog,
                update,
            })
            .map_err(|error| error.to_string())?;
            keys.push(link.key().to_owned());
            link.stage(&mut records);
            cells.insert(cell_id, Cell::Chained(keys));
            replace(&store, &session_id, owned, cells, &records, settle).await
        })
    }

    fn forget(&self, session_id: &str, cell_id: u64) -> HostFuture<'static, ()> {
        let store = self.store.clone();
        let session = self.session(session_id);
        let session_id = session_id.to_owned();
        Box::pin(async move {
            let mut session = session.acquire().await;
            let forgotten = async {
                let (owned, cells, _) = owned(&store, &session_id, &mut session).await?;
                let Some(cell) = cells.remove(&cell_id) else {
                    return Ok(());
                };
                let records = blanked(cell_id, cell);
                replace(&store, &session_id, owned, cells, &records, Settle::Ordered).await
            };
            if let Err(error) = forgotten.await {
                nanocode_trace::warn!(cell_id, %error, "a Code Mode cell journal could not be forgotten");
            }
        })
    }

    fn load(&self, session_id: &str) -> HostFuture<'static, Result<Vec<CellJournal>, String>> {
        let store = self.store.clone();
        let session = self.session(session_id);
        let session_id = session_id.to_owned();
        Box::pin(async move {
            let mut session = session.acquire().await;
            let (_, cells, _) = owned(&store, &session_id, &mut session).await?;
            let state = state_id(&session_id);
            let mut journals = Vec::new();
            for (cell_id, cell) in cells.iter() {
                let mut updates = Vec::new();
                match cell {
                    Cell::Numbered(seqs) => {
                        let keys = seqs
                            .iter()
                            .map(|seq| key(*cell_id, *seq))
                            .collect::<Vec<_>>();
                        let values = store
                            .clone()
                            .read_records(&state, &keys)
                            .await
                            .map_err(|error| error.to_string())?;
                        for value in values
                            .into_iter()
                            .flatten()
                            .filter(|value| !value.is_empty())
                        {
                            updates.push(
                                serde_json::from_str::<CellJournalUpdate>(&value)
                                    .map_err(|error| error.to_string())?,
                            );
                        }
                    }
                    Cell::Chained(keys) => {
                        let links = keys
                            .iter()
                            .map(|key| EncodedPayload::by_key(key))
                            .collect::<Vec<_>>();
                        let loaded = EncodedPayload::load_many(&links, &mut store.clone(), &state)
                            .await
                            .map_err(|error| error.to_string())?;
                        for payload in loaded {
                            let mut link: Link =
                                payload.decode().map_err(|error| error.to_string())?;
                            if let (Some(header), Some(catalog)) =
                                (&mut link.update.header, &link.catalog)
                            {
                                header.tools = EncodedPayload::by_key(catalog)
                                    .load(&mut store.clone(), &state)
                                    .await
                                    .and_then(|payload| payload.decode())
                                    .map_err(|error| error.to_string())?;
                            }
                            updates.push(link.update);
                        }
                    }
                }
                journals.extend(CellJournal::fold(updates));
            }
            Ok(journals)
        })
    }
}
