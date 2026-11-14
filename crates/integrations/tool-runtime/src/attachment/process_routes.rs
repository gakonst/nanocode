use super::ProcessRouteStore;
use crate::contract::ToolError;
use nanocode_sync::{Actor, Atom};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionMapping {
    pub original: i64,
    pub namespace: Option<Box<str>>,
    pub runtime_id: Option<Box<str>>,
}

#[derive(Default, Serialize, Deserialize)]
struct RouteSession {
    next_id: i32,
    handles: HashMap<i32, SessionMapping>,
    #[serde(default)]
    mounts: HashSet<String>,
}

struct RouteOwner {
    session_id: String,
    store: Option<Arc<dyn ProcessRouteStore>>,
    state: Option<RouteSession>,
}

pub(crate) struct ProcessRoutes {
    sessions: Atom<HashMap<String, Actor<RouteOwner>>>,
    store: Option<Arc<dyn ProcessRouteStore>>,
    runtime_id: std::sync::OnceLock<Box<str>>,
}

async fn load<'state>(
    session_id: &str,
    store: Option<&Arc<dyn ProcessRouteStore>>,
    loaded: &'state mut Option<RouteSession>,
) -> Result<&'state mut RouteSession, ToolError> {
    if loaded.is_none() {
        let state: RouteSession = if let Some(store) = store {
            store
                .load(session_id)
                .await?
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default()
        } else {
            RouteSession::default()
        };
        if state.next_id < 0
            || state
                .mounts
                .iter()
                .any(|name| super::RemoteRunner::new(name).is_err())
            || state.handles.iter().any(|(id, mapping)| {
                *id <= 0
                    || *id > state.next_id
                    || mapping.runtime_id.as_deref().is_none_or(str::is_empty)
                    || mapping.namespace.as_deref().is_some_and(str::is_empty)
            })
        {
            return Err("invalid persisted process route journal".into());
        }
        *loaded = Some(state);
    }
    loaded
        .as_mut()
        .ok_or_else(|| "process route session missing".into())
}

fn stopped(_: nanocode_sync::Stopped) -> ToolError {
    "process route session stopped".into()
}

impl ProcessRoutes {
    pub fn new(store: Option<Arc<dyn ProcessRouteStore>>) -> Self {
        Self {
            sessions: Atom::default(),
            store,
            runtime_id: std::sync::OnceLock::new(),
        }
    }

    pub fn runtime_id(&self) -> &str {
        self.runtime_id
            .get_or_init(|| nanocode_platform::uuid_v4().to_string().into())
    }

    fn session(&self, session_id: &str) -> Result<Actor<RouteOwner>, ToolError> {
        if let Some(session) = self
            .sessions
            .read(|sessions| sessions.get(session_id).cloned())
        {
            return Ok(session);
        }
        let session = Actor::spawn(RouteOwner {
            session_id: session_id.to_owned(),
            store: self.store.clone(),
            state: None,
        })?;
        Ok(self.sessions.update(|sessions| {
            sessions
                .entry(session_id.to_owned())
                .or_insert_with(|| session.clone())
                .clone()
        }))
    }

    pub fn remember_mounts<'call>(
        &'call self,
        session_id: &'call str,
        mounts: &'call [String],
    ) -> nanocode_sync::future::BoxFuture<'call, Result<(), ToolError>> {
        Box::pin(async move {
            let mounts = mounts.to_vec();
            self.session(session_id)?
                .run(move |owner| {
                    Box::pin(async move {
                        let RouteOwner {
                            session_id,
                            store,
                            state,
                        } = owner;
                        let state = load(session_id, store.as_ref(), state).await?;
                        let added = mounts
                            .into_iter()
                            .filter(|mount| !state.mounts.contains(mount))
                            .collect::<Vec<_>>();
                        if !added.is_empty() {
                            state.mounts.extend(added.iter().cloned());
                            if let Some(store) = store
                                && let Err(error) =
                                    store.save(session_id, serde_json::to_value(&*state)?).await
                            {
                                for mount in &added {
                                    state.mounts.remove(mount);
                                }
                                return Err(error);
                            }
                        }
                        Ok(())
                    })
                })
                .await
                .map_err(stopped)?
        })
    }

    pub fn known_mount<'call>(
        &'call self,
        session_id: &'call str,
        path: &'call std::path::Path,
    ) -> nanocode_sync::future::BoxFuture<'call, Result<Option<String>, ToolError>> {
        Box::pin(async move {
            let path = PathBuf::from(path);
            self.session(session_id)?
                .run(move |owner| {
                    Box::pin(async move {
                        let state =
                            load(&owner.session_id, owner.store.as_ref(), &mut owner.state).await?;
                        Ok(state
                            .mounts
                            .iter()
                            .map(String::as_str)
                            .chain(
                                state
                                    .handles
                                    .values()
                                    .filter_map(|mapping| mapping.namespace.as_deref()),
                            )
                            .find(|name| path.starts_with(format!("/{name}")))
                            .map(str::to_owned))
                    })
                })
                .await
                .map_err(stopped)?
        })
    }

    pub fn get<'call>(
        &'call self,
        session_id: &'call str,
        id: i32,
    ) -> nanocode_sync::future::BoxFuture<'call, Result<SessionMapping, ToolError>> {
        Box::pin(async move {
            self.session(session_id)?
                .run(move |owner| {
                    Box::pin(async move {
                        load(&owner.session_id, owner.store.as_ref(), &mut owner.state)
                            .await?
                            .handles
                            .get(&id)
                            .cloned()
                            .ok_or_else(|| {
                                format!("write_stdin failed: Unknown process id {id}").into()
                            })
                    })
                })
                .await
                .map_err(stopped)?
        })
    }

    pub fn insert<'call>(
        &'call self,
        session_id: &'call str,
        mapping: SessionMapping,
    ) -> nanocode_sync::future::BoxFuture<'call, Result<i32, ToolError>> {
        Box::pin(async move {
            self.session(session_id)?
                .run(move |owner| {
                    Box::pin(async move {
                        let RouteOwner {
                            session_id,
                            store,
                            state,
                        } = owner;
                        let state = load(session_id, store.as_ref(), state).await?;
                        if let Some((id, _)) = state
                            .handles
                            .iter()
                            .find(|(_, existing)| **existing == mapping)
                        {
                            return Ok(*id);
                        }
                        let id = state
                            .next_id
                            .checked_add(1)
                            .ok_or("process identifier space exhausted")?;
                        state.handles.insert(id, mapping);
                        state.next_id = id;
                        if let Some(store) = store
                            && let Err(error) =
                                store.save(session_id, serde_json::to_value(&*state)?).await
                        {
                            state.handles.remove(&id);
                            return Err(format!(
                                "process executed but its route could not be persisted: {error}"
                            )
                            .into());
                        }
                        Ok(id)
                    })
                })
                .await
                .map_err(stopped)?
        })
    }
}
