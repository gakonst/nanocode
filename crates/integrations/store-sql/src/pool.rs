use nanocode_sync::Atom;
use sqlx::{AssertSqlSafe, Error, PgConnection, PgPool, Postgres, pool::PoolConnection, raw_sql};
use std::{
    ops::{Deref, DerefMut},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    runtime::Handle,
    time::{sleep, timeout},
};

pub struct Lending {
    pool: PgPool,
    within: Duration,
    acquiring: AtomicU32,
    out: AtomicU32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub open: u32,
    pub idle: u32,
    pub out: u32,
    pub acquiring: u32,
    pub max: u32,
}

impl Counts {
    #[must_use]
    pub const fn permits(self) -> u32 {
        self.max.saturating_sub(self.out)
    }

    #[must_use]
    pub const fn owed(self) -> u32 {
        if self.acquiring == 0 {
            return 0;
        }
        let room = self.idle + self.max.saturating_sub(self.open);
        if room < self.acquiring {
            room
        } else {
            self.acquiring
        }
    }
}

impl Lending {
    #[must_use]
    pub fn new(pool: PgPool, within: Duration) -> Arc<Self> {
        Arc::new(Self {
            pool,
            within,
            acquiring: AtomicU32::new(0),
            out: AtomicU32::new(0),
        })
    }

    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn warm(&self, within: Duration, failed: impl FnOnce(&Error)) -> Warmed {
        warm(&self.pool, within, failed).await
    }

    #[must_use]
    pub fn counts(&self) -> Counts {
        Counts {
            open: self.pool.size(),
            idle: u32::try_from(self.pool.num_idle()).unwrap_or(u32::MAX),
            out: self.out.load(Ordering::SeqCst),
            acquiring: self.acquiring.load(Ordering::SeqCst),
            max: self.pool.options().get_max_connections(),
        }
    }

    pub async fn acquire(self: &Arc<Self>) -> Result<Lent, Error> {
        let waiting = Waiting::new(&self.acquiring);
        let conn = self.pool.acquire().await?;
        drop(waiting);
        self.out.fetch_add(1, Ordering::SeqCst);
        Ok(Lent {
            conn: Some(conn),
            pool: Arc::clone(self),
            open: false,
        })
    }

    fn returned(&self, conn: PoolConnection<Postgres>) {
        self.out.fetch_sub(1, Ordering::SeqCst);
        drop(conn);
    }

    fn closed(&self, mut conn: PoolConnection<Postgres>) {
        conn.close_on_drop();
        self.returned(conn);
    }

    fn restore(self: &Arc<Self>, mut conn: PoolConnection<Postgres>) {
        let Ok(runtime) = Handle::try_current() else {
            return self.closed(conn);
        };
        let pool = Arc::clone(self);
        runtime.spawn(async move {
            let restored = timeout(pool.within, raw_sql("ROLLBACK").execute(&mut *conn)).await;
            if matches!(restored, Ok(Ok(_))) {
                pool.returned(conn);
            } else {
                pool.closed(conn);
            }
        });
    }

    pub async fn retire(self: Arc<Self>, every: Duration) {
        while self.pool.size() > u32::try_from(self.pool.num_idle()).unwrap_or(u32::MAX) {
            sleep(every).await;
        }
        self.pool.close().await;
    }

    pub async fn close_within(self: Arc<Self>, within: Duration) {
        drop(timeout(within, self.pool.close()).await);
    }
}

struct Waiting<'a>(&'a AtomicU32);

impl<'a> Waiting<'a> {
    fn new(count: &'a AtomicU32) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

const HELD: &str = "a lent connection is held until the lent value drops";

pub struct Lent {
    conn: Option<PoolConnection<Postgres>>,
    pool: Arc<Lending>,
    open: bool,
}

impl Lent {
    pub const fn opened(&mut self) {
        self.open = true;
    }

    pub const fn ended(&mut self) {
        self.open = false;
    }

    pub fn discard(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.closed(conn);
        }
    }

    pub async fn begin(&mut self, begin: &str) -> Result<(), Error> {
        self.open = true;
        raw_sql(AssertSqlSafe(begin.to_owned()))
            .execute(&mut **self)
            .await?;
        Ok(())
    }

    pub async fn commit(&mut self, collected: &str) -> Result<Option<String>, Error> {
        use sqlx::Row as _;
        let rows = raw_sql(AssertSqlSafe(format!("{collected};COMMIT")))
            .fetch_all(&mut **self)
            .await?;
        self.open = false;
        Ok(rows
            .first()
            .map(|row| row.try_get::<Option<String>, _>(0))
            .transpose()?
            .flatten())
    }
}

impl Deref for Lent {
    type Target = PgConnection;

    fn deref(&self) -> &PgConnection {
        self.conn.as_deref().expect(HELD)
    }
}

impl DerefMut for Lent {
    fn deref_mut(&mut self) -> &mut PgConnection {
        self.conn.as_deref_mut().expect(HELD)
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            if self.open {
                self.pool.restore(conn);
            } else {
                self.pool.returned(conn);
            }
        }
    }
}

pub struct PoolSlot(Atom<Arc<Lending>>);

impl PoolSlot {
    #[must_use]
    pub fn new(pool: Arc<Lending>) -> Self {
        Self(Atom::new(pool))
    }

    #[must_use]
    pub fn get(&self) -> Arc<Lending> {
        Arc::clone(&self.0.get())
    }

    pub fn set(&self, pool: Arc<Lending>) -> Arc<Lending> {
        Arc::clone(&self.0.set(pool))
    }

    pub async fn acquire(&self) -> Result<Lent, Error> {
        loop {
            let pool = self.get();
            match pool.acquire().await {
                Err(Error::PoolClosed) if !self.get().pool.is_closed() && pool.pool.is_closed() => {
                }
                acquired => return acquired,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warmed {
    Full,
    Failed,
    TimedOut,
}

pub async fn warm(pool: &PgPool, within: Duration, failed: impl FnOnce(&Error)) -> Warmed {
    let held = timeout(
        within,
        futures_util::future::try_join_all(
            (0..pool.options().get_max_connections()).map(|_| pool.acquire()),
        ),
    )
    .await;
    match held {
        Ok(Ok(_)) => Warmed::Full,
        Ok(Err(error)) => {
            failed(&error);
            Warmed::Failed
        }
        Err(_) => Warmed::TimedOut,
    }
}

#[must_use]
pub fn unavailable(error: &Error) -> bool {
    match error {
        Error::Io(_) | Error::Tls(_) | Error::Protocol(_) => true,
        Error::Database(error) => error.code().is_some_and(|code| {
            code.starts_with("08") || matches!(&*code, "57P01" | "57P02" | "57P03" | "25006")
        }),
        _ => false,
    }
}

#[must_use]
pub fn contended(error: &Error) -> bool {
    matches!(error, Error::Database(error) if matches!(error.code().as_deref(), Some("40P01" | "55P03")))
}

#[derive(Clone, Copy, Debug)]
pub struct Reaching {
    pub outage: Duration,
    pub lost: Duration,
    pub stranded: Duration,
}

#[derive(Clone)]
pub struct Reach {
    state: Arc<Atom<Reached>>,
    timing: Reaching,
}

#[derive(Clone, Default)]
struct Reached {
    since: Option<Instant>,
    timed_out: Option<Instant>,
    given: Option<Instant>,
    long_holds: usize,
    owed: Option<Instant>,
    replaced: Option<Instant>,
}

impl Reached {
    fn timing_out(&self, now: Instant, outage: Duration) -> bool {
        self.timed_out
            .is_some_and(|at| now.saturating_duration_since(at) <= outage)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stranded {
    Owed,
    Waits,
}

impl Reach {
    #[must_use]
    pub fn new(timing: Reaching) -> Self {
        Self {
            state: Arc::default(),
            timing,
        }
    }

    pub fn note<T>(&self, asked: Instant, result: &Result<T, Error>) {
        let now = Instant::now();
        match result {
            Ok(_) => self.state.update(|reach| {
                reach.since = None;
                reach.given = Some(now);
            }),
            Err(Error::PoolTimedOut) => self.state.update(|reach| {
                if reach.long_holds == 0 {
                    let from = reach.given.map_or(asked, |given| given.max(asked));
                    reach.since.get_or_insert(from);
                    reach.timed_out = Some(now);
                }
            }),
            Err(_) => {}
        }
    }

    #[must_use]
    pub fn lost(&self) -> bool {
        let now = Instant::now();
        let Reaching { outage, lost, .. } = self.timing;
        self.state.read(|reach| {
            reach.long_holds == 0
                && reach
                    .since
                    .is_some_and(|since| now.saturating_duration_since(since) > lost)
                && reach.timing_out(now, outage)
        })
    }

    pub fn stranded(&self, counts: Counts, answering: bool) -> Option<Stranded> {
        let now = Instant::now();
        let Reaching {
            outage, stranded, ..
        } = self.timing;
        self.state.update(|reach| {
            let reason = if counts.owed() == 0 || !answering {
                reach.owed = None;
                None
            } else {
                let since = *reach.owed.get_or_insert(now);
                (now.saturating_duration_since(since) >= stranded).then_some(Stranded::Owed)
            };
            let reason = reason.or_else(|| {
                let since = reach.since?;
                (reach.long_holds == 0
                    && reach.timing_out(now, outage)
                    && counts.open > 0
                    && counts.idle >= counts.open
                    && !reach.replaced.is_some_and(|at| at >= since)
                    && now.saturating_duration_since(since) >= stranded)
                    .then_some(Stranded::Waits)
            });
            if reason.is_some() {
                reach.owed = None;
                reach.replaced = Some(now);
            }
            reason
        })
    }

    #[must_use]
    pub fn timing_out_for(&self) -> Option<Duration> {
        self.state
            .read(|reach| reach.since)
            .map(|since| since.elapsed())
    }

    #[must_use]
    pub fn long_hold(&self) -> LongHold {
        self.state.update(|reach| {
            reach.long_holds += 1;
            reach.since = None;
        });
        LongHold(self.clone())
    }
}

pub struct LongHold(Reach);

impl Drop for LongHold {
    fn drop(&mut self) {
        self.0.state.update(|reach| {
            reach.long_holds = reach.long_holds.saturating_sub(1);
            reach.since = None;
        });
    }
}

pub struct Owing {
    since: Atom<Option<Instant>>,
}

impl Default for Owing {
    fn default() -> Self {
        Self {
            since: Atom::new(None),
        }
    }
}

impl Owing {
    pub fn owed_for(&self, counts: Counts, answering: bool) -> Option<Duration> {
        let now = Instant::now();
        let owed = counts.owed() > 0 && answering;
        self.since
            .update(|since| {
                *since = owed.then(|| since.unwrap_or(now));
                *since
            })
            .map(|since| now.saturating_duration_since(since))
    }

    pub fn forget(&self) {
        self.since.set(None);
    }
}
