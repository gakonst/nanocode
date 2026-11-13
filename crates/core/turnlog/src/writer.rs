use std::sync::Arc;

use nanocode_durability_core::StoreError;
use nanocode_sync::{Held, Owned};

use crate::{
    Fold, LogHost, Record,
    codec::{self, History, PART, RECORDS, SNAPSHOT},
};

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
    pub plain_bytes: u64,
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
    }
}

#[derive(Debug)]
pub struct Row {
    pub bytes: usize,
    pub plain: String,
    pub seq: u64,
    pub snapshot: bool,
}

impl Row {
    pub fn records(&self) -> Result<Vec<Record>, StoreError> {
        crate::decode(&self.plain)
    }
}

pub fn rows(stored: &[(u64, String)]) -> Result<Vec<Row>, StoreError> {
    let mut logical: Vec<(u64, char, String, usize)> = Vec::new();
    for (seq, body) in stored {
        let mut chars = body.chars();
        match chars.next() {
            Some(PART) => match logical.last_mut() {
                Some((_, _, text, bytes)) => {
                    text.push_str(chars.as_str());
                    *bytes += body.len();
                }
                None => {
                    return Err(StoreError::Backend(format!(
                        "turn log row {seq} continues no row"
                    )));
                }
            },
            Some(tag @ (RECORDS | SNAPSHOT)) => {
                logical.push((*seq, tag, chars.as_str().to_owned(), body.len()));
            }
            _ => {
                return Err(StoreError::Backend(format!(
                    "turn log row {seq} has no known tag"
                )));
            }
        }
    }
    let mut history = History::default();
    let mut decoded = Vec::with_capacity(logical.len());
    for (seq, tag, text, bytes) in logical {
        let snapshot = tag == SNAPSHOT;
        if snapshot {
            history.clear();
        }
        let plain = codec::unpack(&text, history.bytes())?;
        history.extend(&plain);
        let plain = String::from_utf8(plain).map_err(|error| {
            StoreError::Backend(format!("turn log row {seq} is not UTF-8: {error}"))
        })?;
        decoded.push(Row {
            bytes,
            plain,
            seq,
            snapshot,
        });
    }
    Ok(decoded)
}

struct Writer {
    buffer: Vec<Record>,
    check: usize,
    floor: Option<u64>,
    fold: Fold,
    history: History,
    next: u64,
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
        let stored = host.load(&scope).await?;
        let next = stored.last().map_or(1, |(seq, _)| seq + 1);
        let mut fold = Fold::default();
        let mut history = History::default();
        let (mut snapshot, mut tail) = (0, 0);
        for row in rows(&stored)? {
            if row.snapshot {
                fold = Fold::default();
                history.clear();
                snapshot = row.bytes;
                tail = 0;
            } else {
                tail += row.bytes;
            }
            fold.load(row.seq, &row.plain)?;
            history.extend(row.plain.as_bytes());
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
                history,
                next,
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
        let writer = &mut *self.writer;
        let seq = writer.next;
        let previous = writer.fold.seq();
        let recovered = writer
            .buffer
            .iter()
            .filter_map(|record| match record {
                Record::Recovery { turn, .. } => Some(turn.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let floor = writer.floor.map(|seq| Record::Floor { seq });
        let plain = crate::encode(
            &floor
                .iter()
                .chain(&writer.buffer)
                .cloned()
                .collect::<Vec<_>>(),
        )?;
        let body = codec::pack(RECORDS, plain.as_bytes(), writer.history.bytes())?;
        let mut snapshot = None;
        if writer.tail + body.len() >= writer.check {
            for turn in &recovered {
                writer.fold.recovered_at(turn, seq);
            }
            writer.fold.committed(seq);
            let encoded = writer.fold.snapshot();
            writer.fold.committed(previous);
            let encoded = encoded?;
            let state = codec::pack(SNAPSHOT, encoded.as_bytes(), &[])?;
            if writer.snapshot + writer.tail + body.len() > 2 * state.len() {
                snapshot = Some((state, encoded));
            } else {
                writer.check = writer
                    .snapshot
                    .max((2 * state.len()).saturating_sub(writer.snapshot));
            }
        }
        let written = snapshot
            .as_ref()
            .map_or(body.as_str(), |(state, _)| state.as_str());
        let parts = codec::parts(written, self.limit)?;
        let stored = parts
            .iter()
            .enumerate()
            .map(|(index, part)| {
                let row = seq + index as u64;
                if index == 0 {
                    (row, (*part).to_owned())
                } else {
                    (row, format!("{PART}{part}"))
                }
            })
            .collect::<Vec<_>>();
        let bytes = stored.iter().map(|(_, body)| body.len()).sum::<usize>();
        self.host.append(self.scope, &stored).await?;
        let writer = &mut *self.writer;
        writer.next = seq + stored.len() as u64;
        writer.fold.committed(seq);
        for turn in &recovered {
            writer.fold.recovered_at(turn, seq);
        }
        writer.buffer.clear();
        writer.floor = None;
        writer.stats.parts += stored.len() as u64 - 1;
        writer.stats.plain_bytes += plain.len() as u64;
        match snapshot {
            Some((state, encoded)) => {
                writer.history.clear();
                writer.history.extend(encoded.as_bytes());
                writer.snapshot = bytes;
                writer.check = bytes;
                writer.tail = 0;
                writer.stats.snapshots += 1;
                writer.stats.snapshot_bytes += state.len() as u64;
            }
            None => {
                writer.history.extend(plain.as_bytes());
                writer.tail += bytes;
                writer.stats.row_bytes += bytes as u64;
            }
        }
        match point {
            Point::Admission => writer.stats.admissions += 1,
            Point::Call => writer.stats.calls += 1,
            Point::Cancel => writer.stats.cancels += 1,
            Point::Finish => writer.stats.finishes += 1,
            Point::Floor => writer.stats.floors += 1,
            Point::Progress => writer.stats.progress += 1,
            Point::Recovery => writer.stats.recoveries += 1,
        }
        Ok(seq)
    }
}
