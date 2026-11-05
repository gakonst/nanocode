use crate::Observe;
use futures_util::future::BoxFuture;
use nanocode_sync::{Lines, Owned, Turn};
use std::{
    fmt::{Debug, Display},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    spawn,
    task::JoinHandle,
    time::{sleep, timeout, timeout_at},
};

pub trait Locker: Send + Sync + 'static {
    type Connection: Send + 'static;
    type Error: Debug + Display + Send + 'static;

    fn connect(&self) -> BoxFuture<'_, Result<Self::Connection, Self::Error>>;

    fn try_all<'a>(
        &'a self,
        connection: &'a mut Self::Connection,
        keys: &'a [String],
    ) -> BoxFuture<'a, Result<bool, Self::Error>>;

    fn holds<'a>(
        &'a self,
        connection: &'a mut Self::Connection,
        keys: &'a [String],
    ) -> BoxFuture<'a, Result<bool, Self::Error>>;

    fn ping<'a>(
        &'a self,
        connection: &'a mut Self::Connection,
    ) -> BoxFuture<'a, Result<(), Self::Error>>;

    fn release<'a>(
        &'a self,
        connection: &'a mut Self::Connection,
    ) -> BoxFuture<'a, Result<(), Self::Error>>;

    fn discard(&self, connection: &mut Self::Connection);
}

#[derive(Debug, thiserror::Error)]
pub enum SessionLockError<E> {
    #[error("waiting for the {kind} session lock passed its {deadline:?} deadline")]
    Late {
        kind: &'static str,
        deadline: Duration,
    },
    #[error("no session lock connection came free for {kind} within {wait:?}")]
    Starved { kind: &'static str, wait: Duration },
    #[error("{0}")]
    Backend(E),
}

pub struct SessionLocks<L> {
    locker: Arc<L>,
    lines: Lines,
    poll: Duration,
    keepalive: Duration,
    try_wait: Duration,
    observe: Arc<dyn Observe>,
}

impl<L: Locker> SessionLocks<L> {
    #[must_use]
    pub fn new(
        locker: L,
        (poll, keepalive, try_wait): (Duration, Duration, Duration),
        observe: Arc<dyn Observe>,
    ) -> Self {
        Self {
            locker: Arc::new(locker),
            lines: Lines::default(),
            poll,
            keepalive,
            try_wait,
            observe,
        }
    }

    pub async fn lock(
        &self,
        kind: &'static str,
        keys: &[&str],
        deadline: Duration,
    ) -> Result<SessionLock<L>, SessionLockError<L::Error>> {
        let asked = Instant::now();
        let until = tokio::time::Instant::from_std(asked + deadline);
        let waiting = Gauge::new(&self.observe, kind, "waiting");
        let keys = sorted(keys);
        let late = || {
            self.observe.session_lock_late(kind, asked.elapsed());
            SessionLockError::Late { kind, deadline }
        };
        let mut queued = Vec::with_capacity(keys.len());
        for key in &keys {
            queued.push(
                timeout_at(until, self.lines.enter(key))
                    .await
                    .map_err(|_| late())?,
            );
        }
        loop {
            let connection = timeout_at(until, self.locker.connect())
                .await
                .map_err(|_| late())?
                .map_err(SessionLockError::Backend)?;
            if let Some(lock) = self.taken(connection, kind, &keys, &mut queued).await? {
                drop(waiting);
                self.observe.session_lock_waited(kind, asked.elapsed());
                return Ok(lock);
            }
            let now = tokio::time::Instant::now();
            if now >= until {
                return Err(late());
            }
            sleep(self.poll.min(until - now)).await;
        }
    }

    pub async fn try_lock(
        &self,
        kind: &'static str,
        keys: &[&str],
    ) -> Result<Option<SessionLock<L>>, SessionLockError<L::Error>> {
        let keys = sorted(keys);
        let mut queued = Vec::with_capacity(keys.len());
        for key in &keys {
            match self.lines.try_enter(key) {
                Some(turn) => queued.push(turn),
                None => return Ok(None),
            }
        }
        let connection = timeout(self.try_wait, self.locker.connect())
            .await
            .map_err(|_| {
                self.observe.session_lock_starved(kind);
                SessionLockError::Starved {
                    kind,
                    wait: self.try_wait,
                }
            })?
            .map_err(SessionLockError::Backend)?;
        self.taken(connection, kind, &keys, &mut queued).await
    }

    async fn taken(
        &self,
        mut connection: L::Connection,
        kind: &'static str,
        keys: &[String],
        queued: &mut Vec<Turn>,
    ) -> Result<Option<SessionLock<L>>, SessionLockError<L::Error>> {
        if !self
            .locker
            .try_all(&mut connection, keys)
            .await
            .map_err(SessionLockError::Backend)?
        {
            return Ok(None);
        }
        let connection = Arc::new(Owned::new(connection));
        let lost = Arc::new(AtomicBool::new(false));
        let keepalive = {
            let (connection, lost) = (Arc::clone(&connection), Arc::clone(&lost));
            let (locker, observe, every) = (
                Arc::clone(&self.locker),
                Arc::clone(&self.observe),
                self.keepalive,
            );
            spawn(async move {
                loop {
                    sleep(every).await;
                    let pinged = locker.ping(&mut *connection.acquire().await).await;
                    if let Err(error) = pinged {
                        observe.session_lock_lost(kind, &error);
                        lost.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            })
        };
        Ok(Some(SessionLock {
            locker: Arc::clone(&self.locker),
            connection,
            keys: keys.to_vec(),
            kind,
            lost,
            keepalive,
            queued: std::mem::take(queued),
            observe: Arc::clone(&self.observe),
            _held: Gauge::new(&self.observe, kind, "held"),
        }))
    }
}

pub struct SessionLock<L: Locker> {
    locker: Arc<L>,
    connection: Arc<Owned<L::Connection>>,
    keys: Vec<String>,
    kind: &'static str,
    lost: Arc<AtomicBool>,
    keepalive: JoinHandle<()>,
    queued: Vec<Turn>,
    observe: Arc<dyn Observe>,
    _held: Gauge,
}

impl<L: Locker> SessionLock<L> {
    pub async fn holds(&self) -> Result<bool, L::Error> {
        if self.lost.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let mut connection = self.connection.acquire().await;
        self.locker.holds(&mut connection, &self.keys).await
    }
}

impl<L: Locker> Drop for SessionLock<L> {
    fn drop(&mut self) {
        self.keepalive.abort();
        let (locker, connection, queued) = (
            Arc::clone(&self.locker),
            Arc::clone(&self.connection),
            std::mem::take(&mut self.queued),
        );
        let (kind, observe) = (self.kind, Arc::clone(&self.observe));
        spawn(async move {
            let mut connection = connection.acquire().await;
            if let Err(error) = locker.release(&mut connection).await {
                observe.session_lock_unreleased(kind, &error);
                locker.discard(&mut connection);
            }
            drop(queued);
        });
    }
}

struct Gauge(Arc<dyn Observe>, &'static str, &'static str);

impl Gauge {
    fn new(observe: &Arc<dyn Observe>, kind: &'static str, state: &'static str) -> Self {
        observe.session_lock_state(kind, state, 1.0);
        Self(Arc::clone(observe), kind, state)
    }
}

impl Drop for Gauge {
    fn drop(&mut self) {
        self.0.session_lock_state(self.1, self.2, -1.0);
    }
}

fn sorted(keys: &[&str]) -> Vec<String> {
    let mut keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
    keys.sort();
    keys.dedup();
    keys
}
