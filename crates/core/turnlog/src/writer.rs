use std::sync::Arc;

use nanocode_durability_core::StoreError;
use nanocode_sync::{Held, Owned};

use crate::{Fold, LogHost, Record};

pub const EVENT_GAP: u64 = 1 << 14;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Point {
    Admission,
    Call,
    Cancel,
    Finish,
    Floor,
    Progress,
    Recovery,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub admissions: u64,
    pub calls: u64,
    pub cancels: u64,
    pub finishes: u64,
    pub floors: u64,
    pub parts: u64,
    pub progress: u64,
    pub recoveries: u64,
    pub row_bytes: u64,
    pub snapshot_bytes: u64,
    pub snapshots: u64,
}

impl Stats {
    #[must_use]
    pub const fn rows(&self) -> u64 {
        self.admissions
            + self.calls
            + self.cancels
            + self.finishes
            + self.floors
            + self.parts
            + self.progress
            + self.recoveries
            + self.snapshots
    }
}

struct Writer {
    buffer: Vec<Record>,
    check: usize,
    floor: Option<u64>,
    fold: Fold,
    snapshot: usize,
    stats: Stats,
    tail: usize,
}

pub struct TurnLog {
    host: Arc<dyn LogHost>,
    limit: Option<usize>,
    scope: Arc<str>,
    writer: Owned<Writer>,
}

pub struct Lease<'a> {
    host: &'a dyn LogHost,
    limit: Option<usize>,
    scope: &'a str,
    writer: Held<'a, Writer>,
}

impl TurnLog {
    pub async fn open(
        host: Arc<dyn LogHost>,
        scope: impl Into<Arc<str>>,
    ) -> Result<Self, StoreError> {
        let scope = scope.into();
        let mut fold = Fold::default();
        let (after, snapshot) = match host.snapshot(&scope).await? {
            Some((seq, body)) => {
                fold.load(seq, &body)?;
                (seq, body.len())
            }
            None => (0, 0),
        };
        let mut tail = 0;
        for (seq, body) in host.scan(&scope, after).await? {
            fold.load(seq, &body)?;
            tail += body.len();
        }
        fold.reopen(EVENT_GAP);
        let floor = Some(fold.last_event());
        Ok(Self {
            host,
            limit: None,
            scope,
            writer: Owned::new(Writer {
                buffer: Vec::new(),
                check: snapshot,
                floor,
                fold,
                snapshot,
                stats: Stats::default(),
                tail,
            }),
        })
    }

    #[must_use]
    pub const fn row_limit(mut self, limit: Option<usize>) -> Self {
        self.limit = limit;
        self
    }

    #[must_use]
    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub async fn lease(&self) -> Lease<'_> {
        Lease {
            host: &*self.host,
            limit: self.limit,
            scope: &self.scope,
            writer: self.writer.acquire().await,
        }
    }

    pub async fn read<R>(&self, read: impl FnOnce(&Fold) -> R) -> R {
        read(&self.writer.acquire().await.fold)
    }

    pub async fn stats(&self) -> Stats {
        self.writer.acquire().await.stats
    }
}

impl Lease<'_> {
    #[must_use]
    pub fn fold(&self) -> &Fold {
        &self.writer.fold
    }

    #[must_use]
    pub fn buffered(&self) -> usize {
        self.writer.buffer.len()
    }

    pub fn next_event(&mut self) -> u64 {
        self.writer.fold.next_event()
    }

    pub fn push(&mut self, record: Record) -> Result<(), StoreError> {
        let writer = &mut *self.writer;
        writer.fold.apply(&record, None)?;
        if let Record::Checkpoint { state, .. } = &record {
            writer.buffer.retain(
                |buffered| !matches!(buffered, Record::Checkpoint { state: other, .. } if other == state),
            );
        }
        writer.buffer.push(record);
        Ok(())
    }

    #[must_use]
    pub fn floor_pending(&self) -> bool {
        self.writer.floor.is_some()
    }

    pub async fn secure(&mut self) -> Result<(), StoreError> {
        if self.writer.floor.is_some() {
            self.commit(Point::Floor).await?;
        }
        Ok(())
    }

    pub async fn commit(&mut self, point: Point) -> Result<u64, StoreError> {
        let floor = self.writer.floor.map(|seq| Record::Floor { seq });
        let parts = crate::split(floor.iter().chain(&self.writer.buffer), self.limit)?;
        let last = parts.len() - 1;
        let mut seq = self.writer.fold.seq();
        for (index, (body, count)) in parts.iter().enumerate() {
            let row = self.host.append(self.scope, body).await?;
            let writer = &mut *self.writer;
            if row <= seq {
                return Err(StoreError::Backend(format!(
                    "turn log host appended row {row} after row {seq}"
                )));
            }
            seq = row;
            writer.fold.committed(row);
            let records = count - usize::from(writer.floor.take().is_some());
            for record in writer.buffer.drain(..records) {
                if let Record::Recovery { turn, .. } = record {
                    writer.fold.recovered_at(&turn, row);
                }
            }
            writer.tail += body.len();
            writer.stats.row_bytes += body.len() as u64;
            if index < last {
                writer.stats.parts += 1;
            }
        }
        let writer = &mut *self.writer;
        match point {
            Point::Admission => writer.stats.admissions += 1,
            Point::Call => writer.stats.calls += 1,
            Point::Cancel => writer.stats.cancels += 1,
            Point::Finish => writer.stats.finishes += 1,
            Point::Floor => writer.stats.floors += 1,
            Point::Progress => writer.stats.progress += 1,
            Point::Recovery => writer.stats.recoveries += 1,
        }
        if writer.tail >= writer.check {
            match writer.fold.snapshot() {
                Ok(snapshot) if self.limit.is_some_and(|limit| snapshot.len() > limit) => {
                    tracing::warn!(
                        scope = self.scope,
                        bytes = snapshot.len(),
                        "turn log snapshot exceeds the row limit; the log tail keeps the state"
                    );
                    writer.check = writer.tail.saturating_add(snapshot.len());
                }
                Ok(snapshot) if writer.snapshot + writer.tail > 2 * snapshot.len() => {
                    match self.host.put_snapshot(self.scope, seq, &snapshot).await {
                        Ok(()) => {
                            let writer = &mut *self.writer;
                            writer.snapshot = snapshot.len();
                            writer.check = snapshot.len();
                            writer.tail = 0;
                            writer.stats.snapshots += 1;
                            writer.stats.snapshot_bytes += snapshot.len() as u64;
                        }
                        Err(error) => {
                            tracing::warn!(%error, scope = self.scope, seq, "turn log snapshot was not written; the next row retries it");
                        }
                    }
                }
                Ok(snapshot) => {
                    let writer = &mut *self.writer;
                    writer.check = writer
                        .snapshot
                        .max((2 * snapshot.len()).saturating_sub(writer.snapshot));
                }
                Err(error) => {
                    tracing::warn!(%error, scope = self.scope, seq, "turn log snapshot was not encoded; the next row retries it");
                }
            }
        }
        Ok(seq)
    }
}
