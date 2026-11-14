use crate::postgres::{PgConnection, PgError, PgOptions, connection::Reuse};
use nanocode_sync::{Leased, Owned};
use nanocode_sync::{OwnedSemaphorePermit, Semaphore};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[derive(Clone)]
pub struct PgPool(Arc<Shared>);

struct Shared {
    options: PgOptions,
    reuse: Reuse,
    slots: Box<[Arc<Owned<Option<PgConnection>>>]>,
    permits: Arc<Semaphore>,
}

pub struct Pooled {
    slot: Leased<Option<PgConnection>>,
    _permit: OwnedSemaphorePermit,
}

impl PgPool {
    #[must_use]
    pub fn new(options: PgOptions, connections: usize) -> Self {
        assert!(connections > 0, "a pool holds at least one connection");
        Self(Arc::new(Shared {
            options,
            reuse: Reuse::default(),
            slots: (0..connections)
                .map(|_| Arc::new(Owned::new(None)))
                .collect(),
            permits: Arc::new(Semaphore::new(connections)),
        }))
    }

    #[must_use]
    pub fn connections(&self) -> usize {
        self.0.slots.len()
    }

    #[must_use]
    pub fn options(&self) -> &PgOptions {
        &self.0.options
    }

    pub async fn acquire(&self) -> Result<Pooled, PgError> {
        let permit = Arc::clone(&self.0.permits)
            .acquire_owned()
            .await
            .expect("a pool never closes its semaphore");
        let slot = self
            .0
            .slots
            .iter()
            .find_map(|slot| Arc::clone(slot).try_acquire_owned())
            .expect("a permit leaves a slot free");
        let mut pooled = Pooled {
            slot,
            _permit: permit,
        };
        if !pooled.slot.as_ref().is_some_and(PgConnection::is_ready) {
            *pooled.slot = None;
            *pooled.slot =
                Some(PgConnection::connect_reusing(&self.0.options, &self.0.reuse).await?);
        }
        Ok(pooled)
    }
}

impl Deref for Pooled {
    type Target = PgConnection;

    fn deref(&self) -> &PgConnection {
        self.slot
            .as_ref()
            .expect("a pooled connection is present while it is lent")
    }
}

impl DerefMut for Pooled {
    fn deref_mut(&mut self) -> &mut PgConnection {
        self.slot
            .as_mut()
            .expect("a pooled connection is present while it is lent")
    }
}
