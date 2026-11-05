use crate::{Lines, Owned, Turn};
use futures_util::future::BoxFuture;
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

    fn state(&self, _kind: &'static str, _state: &'static str, _change: f64) {}

    fn late(&self, _kind: &'static str, _waited: Duration) {}

    fn waited(&self, _kind: &'static str, _waited: Duration) {}

    fn starved(&self, _kind: &'static str) {}

    fn lost(&self, _kind: &'static str, _error: &Self::Error) {}

    fn unreleased(&self, _kind: &'static str, _error: &Self::Error) {}
}

#[derive(Debug)]
pub enum SessionLockError<E> {
    Late {
        kind: &'static str,
        deadline: Duration,
    },
    Starved {
        kind: &'static str,
        wait: Duration,
    },
    Backend(E),
}

impl<E: Display> Display for SessionLockError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Late { kind, deadline } => write!(
                formatter,
                "waiting for the {kind} session lock passed its {deadline:?} deadline"
            ),
            Self::Starved { kind, wait } => write!(
                formatter,
                "no session lock connection came free for {kind} within {wait:?}"
            ),
            Self::Backend(error) => error.fmt(formatter),
        }
    }
}

impl<E: Debug + Display> std::error::Error for SessionLockError<E> {}

pub struct SessionLocks<L> {
    locker: Arc<L>,
    lines: Lines,
    poll: Duration,
    keepalive: Duration,
    try_wait: Duration,
}

impl<L: Locker> SessionLocks<L> {
    #[must_use]
    pub fn new(locker: L, (poll, keepalive, try_wait): (Duration, Duration, Duration)) -> Self {
        Self {
            locker: Arc::new(locker),
            lines: Lines::default(),
            poll,
            keepalive,
            try_wait,
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
        let waiting = Gauge::new(&self.locker, kind, "waiting");
        let keys = sorted(keys);
        let late = || {
            self.locker.late(kind, asked.elapsed());
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
                self.locker.waited(kind, asked.elapsed());
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
                self.locker.starved(kind);
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
            let (locker, every) = (Arc::clone(&self.locker), self.keepalive);
            spawn(async move {
                loop {
                    sleep(every).await;
                    let pinged = locker.ping(&mut *connection.acquire().await).await;
                    if let Err(error) = pinged {
                        locker.lost(kind, &error);
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
            _held: Gauge::new(&self.locker, kind, "held"),
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
    _held: Gauge<L>,
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
        let kind = self.kind;
        spawn(async move {
            let mut connection = connection.acquire().await;
            if let Err(error) = locker.release(&mut connection).await {
                locker.unreleased(kind, &error);
                locker.discard(&mut connection);
            }
            drop(queued);
        });
    }
}

struct Gauge<L: Locker>(Arc<L>, &'static str, &'static str);

impl<L: Locker> Gauge<L> {
    fn new(locker: &Arc<L>, kind: &'static str, state: &'static str) -> Self {
        locker.state(kind, state, 1.0);
        Self(Arc::clone(locker), kind, state)
    }
}

impl<L: Locker> Drop for Gauge<L> {
    fn drop(&mut self) {
        self.0.state(self.1, self.2, -1.0);
    }
}

fn sorted(keys: &[&str]) -> Vec<String> {
    let mut keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
    keys.sort();
    keys.dedup();
    keys
}
