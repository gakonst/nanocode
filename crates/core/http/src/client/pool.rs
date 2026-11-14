use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use nanocode_io::ReadBuf;
use nanocode_platform::{Instant, now};
use nanocode_sync::{Atom, Slot};

use crate::wire::Connection;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key {
    pub(crate) secure: bool,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) proxy: Option<String>,
}

#[derive(Clone)]
struct Idle {
    connection: Arc<Slot<Connection>>,
    since: Instant,
}

pub(crate) struct Pool {
    idle: Atom<BTreeMap<Key, Vec<Idle>>>,
    idle_per_host_max: usize,
    idle_timeout: Option<Duration>,
}

impl Pool {
    pub(crate) fn new(idle_per_host_max: usize, idle_timeout: Option<Duration>) -> Self {
        Self {
            idle: Atom::default(),
            idle_per_host_max,
            idle_timeout,
        }
    }

    fn fresh(&self, idle: &Idle, now: Instant) -> bool {
        self.idle_timeout
            .is_none_or(|timeout| now.duration_since(idle.since) < timeout)
    }

    pub(crate) fn checkout(&self, key: &Key) -> Option<Connection> {
        loop {
            let now = now();
            let found = self.idle.update(|idle| {
                let list = idle.get_mut(key)?;
                list.retain(|entry| self.fresh(entry, now));
                let found = list.pop();
                if list.is_empty() {
                    idle.remove(key);
                }
                found
            })?;
            if let Some(mut connection) = found.connection.take()
                && open(&mut connection)
            {
                return Some(connection);
            }
        }
    }

    pub(crate) fn checkin(&self, key: Key, connection: Connection) {
        if self.idle_per_host_max == 0 || self.idle_timeout.is_some_and(|timeout| timeout.is_zero())
        {
            return;
        }
        let entry = Idle {
            connection: Arc::new(Slot::new(Some(connection))),
            since: now(),
        };
        self.idle.update(|idle| {
            let list = idle.entry(key.clone()).or_default();
            if list.len() < self.idle_per_host_max {
                list.push(entry.clone());
            }
        });
    }
}

fn open(connection: &mut Connection) -> bool {
    if !connection.unread().is_empty() {
        return false;
    }
    let mut probe = [0u8; 1];
    let mut buffer = ReadBuf::new(&mut probe);
    let mut context = Context::from_waker(std::task::Waker::noop());
    matches!(
        nanocode_io::AsyncRead::poll_read(Pin::new(connection.io_mut()), &mut context, &mut buffer),
        Poll::Pending
    )
}
