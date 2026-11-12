use nanocode_durability::{OwnedState, OwnerId, SharedStore, StateStore};
use nanocode_sync::Atom;
use nanocode_tool_host::{attachment::ProcessRouteStore, contract::ToolError};
use std::{collections::HashMap, sync::Arc};

pub(crate) struct DurableProcessRoutes {
    store: SharedStore,
    owners: Atom<HashMap<String, OwnedState>>,
}

impl DurableProcessRoutes {
    pub(crate) fn new(store: SharedStore) -> Arc<Self> {
        Arc::new(Self {
            store,
            owners: Atom::default(),
        })
    }
}

impl ProcessRouteStore for DurableProcessRoutes {
    fn load<'life0, 'life1, 'async_trait>(
        &'life0 self,
        session_id: &'life1 str,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = Result<Option<serde_json::Value>, ToolError>> + Send + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Result<Option<serde_json::Value>, ToolError> = {
                let state_id = format!("{session_id}:process-routes");
                let owned = self
                    .store
                    .clone()
                    .acquire(&state_id, OwnerId::new())
                    .await
                    .map_err(|error| -> ToolError { Box::new(error) })?;
                let value = owned
                    .state
                    .payload
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?;
                self.owners.update(|owners| {
                    owners.insert(state_id.clone(), owned.clone());
                });
                Ok(value)
            };
            output
        })
    }

    fn save<'life0, 'life1, 'async_trait>(
        &'life0 self,
        session_id: &'life1 str,
        value: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Result<(), ToolError> = {
                let payload = serde_json::to_string(&value)?;
                let state_id = format!("{session_id}:process-routes");
                let owned = self
                    .owners
                    .read(|owners| owners.get(&state_id).cloned())
                    .ok_or("process routes were not loaded before saving")?;
                let revision = self
                    .store
                    .clone()
                    .replace(&state_id, &owned.owner, owned.state.revision, &payload, &[])
                    .await
                    .map_err(|error| -> ToolError { Box::new(error) })?;
                let saved = OwnedState {
                    owner: owned.owner.clone(),
                    state: nanocode_durability::StoredState {
                        revision,
                        payload: Some(payload),
                    },
                };
                self.owners.update(|owners| {
                    if owners
                        .get(&state_id)
                        .is_some_and(|current| current.owner == owned.owner)
                    {
                        owners.insert(state_id.clone(), saved.clone());
                    }
                });
                Ok(())
            };
            output
        })
    }
}
