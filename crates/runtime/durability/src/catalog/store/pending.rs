use serde_json::{Value, value::RawValue};

use super::SessionStore;
use crate::{EncodedPayload, Error, Result};

#[derive(Clone, Debug)]
pub enum CancelTo {
    Latest,
    Checkpoint(EncodedPayload),
    Fresh,
}

#[derive(Clone, Debug, Default)]
pub struct CancelledPending {
    pub cancelled: usize,
    pub checkpoint: Option<EncodedPayload>,
    pub unresolved: Option<String>,
}

impl SessionStore {
    pub async fn cancel_pending(&self, id: &str, to: CancelTo) -> Result<Option<CancelledPending>> {
        let session = self.resume(id).await?;
        let pending: Vec<(String, EncodedPayload)> = session
            .state()
            .await?
            .pending_operations()
            .into_iter()
            .map(|(operation, state)| (operation.to_owned(), state.input.clone()))
            .collect();
        if pending.is_empty() {
            return Ok(Some(CancelledPending::default()));
        }
        let (owner, latest) = session.acquire_agent().await?;
        let mut unresolved = None;
        let checkpoint = match to {
            CancelTo::Latest => latest,
            CancelTo::Checkpoint(base) => match session.resolve(&base).await {
                Ok(_) => Some(base),
                Err(error) => {
                    unresolved = Some(error.to_string());
                    latest
                }
            },
            CancelTo::Fresh => None,
        };
        let Some(checkpoint) = checkpoint else {
            drop(owner.shutdown().await);
            return Ok(None);
        };
        let cancelled = async {
            for (operation, input) in &pending {
                let input = session.resolve(input).await?;
                let input = RawValue::from_string(input.json()?.to_owned())
                    .map_err(|error| Error::InvalidState(error.to_string()))?;
                owner
                    .admit_typed::<_, Value, Value>(operation.clone(), &*input)
                    .await?;
                owner.begin_attempt(operation.clone()).await?;
                owner
                    .cancel(operation.clone(), Some(checkpoint.clone()))
                    .await?;
            }
            Ok(())
        }
        .await;
        drop(owner.shutdown().await);
        cancelled.map(|()| {
            Some(CancelledPending {
                cancelled: pending.len(),
                checkpoint: Some(checkpoint),
                unresolved,
            })
        })
    }
}
