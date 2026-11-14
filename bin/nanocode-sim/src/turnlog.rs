use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    sync::Arc,
};

use nanocode_durability_core::{StoreError, StoreFuture};
use nanocode_sync::FutureExt;
use nanocode_turnlog::{
    Compaction, EffectClass, LogHost, Outcome as LogOutcome, Phase, Point as LogPoint,
    Record as LogRecord, Status as LogStatus, TurnLog,
};
use serde_json::value::{RawValue, to_raw_value};

use crate::{
    disk::Disk,
    onelog::{
        Appended, Class, Decision, EventId, Kind, Log, Outcome, Point, Protections, RESTART_CAP,
        Record, Reply, ScopeState, Status, TurnDurability, encode,
    },
};

pub struct SimHost {
    disk: Arc<Disk>,
    kept: bool,
}

impl LogHost for SimHost {
    fn scan<'a>(
        &'a self,
        scope: &'a str,
        from: u64,
        to: u64,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>> {
        let rows = self.disk.with(|rows| rows.tscan(scope, from, to));
        Box::pin(async move { Ok(rows) })
    }

    fn load<'a>(
        &'a self,
        scope: &'a str,
    ) -> StoreFuture<'a, Result<Vec<(u64, String)>, StoreError>> {
        let rows = self.disk.with(|rows| rows.tload(scope));
        Box::pin(async move { Ok(rows) })
    }

    fn append<'a>(
        &'a self,
        scope: &'a str,
        rows: &'a [(u64, String)],
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        let inserted = self.disk.with(|disk| disk.insert(scope, rows));
        Box::pin(async move {
            if inserted {
                Ok(())
            } else {
                Err(StoreError::Fenced)
            }
        })
    }

    fn append_compacting<'a>(
        &'a self,
        scope: &'a str,
        rows: &'a [(u64, String)],
        compaction: Compaction<'a>,
    ) -> StoreFuture<'a, Result<(), StoreError>> {
        let kept = self.kept;
        if let Some((first, _)) = compaction.archived.first() {
            let replaced = self.disk.with(|disk| {
                disk.view(scope)
                    .into_iter()
                    .filter(|(seq, _)| *seq >= *first)
                    .take(compaction.archived.len() + compaction.dropped.clone().count())
                    .collect::<Vec<_>>()
            });
            let plain = |stored: &[(u64, String)]| {
                let rows = nanocode_turnlog::rows(stored).unwrap_or_else(|error| {
                    panic!("an archived or replaced turn log row does not decode: {error}")
                });
                assert_eq!(rows.len(), 1, "an archive replaces one snapshot row");
                rows.into_iter()
                    .next()
                    .map(|row| row.plain)
                    .unwrap_or_default()
            };
            let (snapshot, archive) = (plain(&replaced), plain(compaction.archived));
            let events = |text: &str| {
                nanocode_turnlog::decode(text)
                    .unwrap_or_else(|error| panic!("a turn log row does not decode: {error}"))
                    .into_iter()
                    .filter_map(|record| match record {
                        LogRecord::Event { seq, .. } => Some(seq),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            assert!(
                replaced
                    .first()
                    .is_some_and(|(_, body)| body.starts_with('s'))
                    && snapshot.ends_with(&archive)
                    && events(&snapshot) == events(&archive),
                "the archive of {scope} row {first} is not a suffix of its snapshot holding every event"
            );
        }
        let inserted = self.disk.with(|disk| {
            let inserted = disk.insert(scope, rows);
            if inserted && !kept {
                disk.tcompactions.push((
                    scope.to_owned(),
                    compaction.archived.to_vec(),
                    compaction.dropped,
                ));
            }
            inserted
        });
        Box::pin(async move {
            if inserted {
                Ok(())
            } else {
                Err(StoreError::Fenced)
            }
        })
    }
}

fn padding(scope: &str, bytes: usize) -> String {
    let mut seed = scope.bytes().fold(0x9e37_79b9_7f4a_7c15_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    (0..bytes)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            char::from(b'a' + u8::try_from(seed % 26).expect("a letter index fits in u8"))
        })
        .collect()
}

fn ready<T>(future: impl Future<Output = T>) -> T {
    future
        .now_or_never()
        .expect("the simulated turn log host answers every call at once")
}

fn refused<T>(result: Result<T, StoreError>) -> T {
    result.unwrap_or_else(|error| panic!("nanocode-turnlog refused: {error}"))
}

fn raw(text: &str) -> Box<RawValue> {
    to_raw_value(text).expect("a string encodes as JSON")
}

pub fn text(value: &RawValue) -> String {
    serde_json::from_str(value.get()).unwrap_or_else(|_| value.get().to_owned())
}

pub struct TurnLogPolicy {
    bound: Option<usize>,
    disk: Arc<Disk>,
    host: Arc<dyn LogHost>,
    limit: Option<usize>,
    logs: BTreeMap<String, TurnLog>,
    ratio: usize,
    protections: Protections,
    restarting: BTreeMap<String, String>,
    retain: Option<usize>,
    retention: Option<usize>,
    rows: Option<usize>,
    state: Option<usize>,
    unsealed: BTreeSet<String>,
}

impl TurnLogPolicy {
    pub fn new(
        disk: Arc<Disk>,
        protections: Protections,
        limit: Option<usize>,
        ratio: usize,
        retain: Option<usize>,
        retention: Option<usize>,
    ) -> Self {
        Self {
            bound: None,
            disk: Arc::clone(&disk),
            host: Arc::new(SimHost { disk, kept: false }),
            limit,
            logs: BTreeMap::new(),
            ratio,
            protections,
            restarting: BTreeMap::new(),
            retain,
            retention,
            rows: None,
            state: None,
            unsealed: BTreeSet::new(),
        }
    }

    pub fn state_bytes(mut self, bytes: Option<usize>, kept: bool) -> Self {
        self.state = bytes;
        self.host = Arc::new(SimHost {
            disk: Arc::clone(&self.disk),
            kept,
        });
        self
    }

    pub const fn snapshot_rows(mut self, rows: Option<usize>, bound: Option<usize>) -> Self {
        self.rows = rows;
        self.bound = bound;
        self
    }

    fn log(&mut self, scope: &str) -> &TurnLog {
        if !self.logs.contains_key(scope) {
            let log = refused(ready(TurnLog::open(Arc::clone(&self.host), scope)))
                .retain_finished(self.retain)
                .row_limit(self.limit)
                .snapshot_ratio(self.ratio)
                .snapshot_rows(self.rows);
            self.logs.insert(scope.to_owned(), log);
        }
        &self.logs[scope]
    }

    fn push(&mut self, scope: &str, record: LogRecord) {
        let log = self.log(scope);
        refused(ready(log.lease()).push(record));
    }

    fn records(&mut self, scope: &str, record: Record) -> Vec<LogRecord> {
        let restarting = self.restarting.get(scope).cloned();
        let log = self.log(scope);
        let lease = ready(log.lease());
        let fold = lease.fold();
        match record {
            Record::Admitted { origin, request } => vec![
                LogRecord::Admitted {
                    digest: String::new(),
                    first: fold.last_event(),
                    hand: None,
                    metadata: raw(&format!("{origin:?}")),
                    prompt: raw(&request),
                    turn: request.clone(),
                },
                LogRecord::Status {
                    status: LogStatus::Running,
                    turn: request,
                },
            ],
            Record::Ended {
                nonce,
                request,
                status,
            } => vec![LogRecord::Status {
                status: LogStatus::Finished {
                    last: fold.last_event(),
                    outcome: match status {
                        Status::Cancelled => LogOutcome::Cancelled,
                        Status::Failed => LogOutcome::Failed,
                        Status::Finished => LogOutcome::Completed,
                    },
                    result: raw(&format!("{}:{nonce}", status.code())),
                },
                turn: request,
            }],
            Record::Event { id, text } => {
                let Some((0, seq)) = id else {
                    panic!("an event without a turn log seq: {text}");
                };
                vec![LogRecord::Event {
                    at: 0,
                    event: raw(&text),
                    seq,
                    turn: None,
                }]
            }
            Record::Floor { .. } => Vec::new(),
            Record::Model {
                ref reply,
                ref request,
                ..
            } => {
                let mut records = vec![LogRecord::Model {
                    response: raw(encode(std::slice::from_ref(&record)).trim_end()),
                    turn: request.clone(),
                }];
                if let Reply::Calls(calls) = reply {
                    for call in calls {
                        let effect = match call.tool.class() {
                            Class::External => EffectClass::External,
                            Class::Idempotent => EffectClass::Idempotent,
                            Class::InObject => {
                                records.push(LogRecord::Host {
                                    key: format!("effect:{}", call.id),
                                    value: raw(&format!("{:?}", call.tool)),
                                });
                                continue;
                            }
                            Class::None => continue,
                        };
                        records.push(LogRecord::Call {
                            call_id: call.id.clone(),
                            effect,
                            input: raw(&format!("{:?}", call.tool)),
                            tool: format!("{:?}", call.tool),
                            turn: Some(request.clone()),
                        });
                    }
                }
                records
            }
            Record::Restart { attempt, .. } => {
                let turn =
                    restarting.expect("a restart record follows the policy's restart decision");
                vec![
                    LogRecord::Recovery {
                        at: None,
                        attempt,
                        turn: turn.clone(),
                        unknown: fold.unknown(),
                    },
                    LogRecord::Status {
                        status: LogStatus::Running,
                        turn,
                    },
                ]
            }
            Record::Result { call, outcome } => vec![LogRecord::Result {
                call_id: call,
                output: match outcome {
                    Outcome::Done(text) => raw(&text),
                    Outcome::Unknown => raw("unknown"),
                },
            }],
        }
    }

    const fn commit_point(
        &self,
        point: Point,
        recovering: bool,
        floor: bool,
    ) -> Option<(LogPoint, Kind)> {
        let protections = &self.protections;
        match point {
            Point::Admission if protections.admission => {
                Some((LogPoint::Admission, Kind::Admission))
            }
            Point::Calls {
                external,
                in_object,
            } if (external && protections.call_external)
                || (in_object && protections.call_in_object) =>
            {
                Some((LogPoint::Call, Kind::Call))
            }
            Point::Calls { .. } | Point::Results { .. } if protections.progress && recovering => {
                Some((LogPoint::Progress, Kind::Progress))
            }
            Point::Finish if protections.finish => Some((LogPoint::Finish, Kind::Finish)),
            Point::Restart if protections.restart => Some((LogPoint::Recovery, Kind::Restart)),
            Point::Seal if floor => Some((LogPoint::Floor, Kind::Floor)),
            _ => None,
        }
    }
}

impl TurnDurability for TurnLogPolicy {
    fn event_id(&mut self, scope: &str, _state: &mut ScopeState) -> Option<EventId> {
        let log = self.log(scope);
        let mut lease = ready(log.lease());
        let floor = lease.floor_pending();
        let seq = lease.next_event();
        drop(lease);
        if floor {
            self.unsealed.insert(scope.to_owned());
        }
        Some((0, seq))
    }

    fn load(&mut self, _log: &mut dyn Log, scope: &str) -> ScopeState {
        let log = refused(ready(TurnLog::open(Arc::clone(&self.host), scope)))
            .retain_finished(self.retain)
            .row_limit(self.limit)
            .snapshot_ratio(self.ratio)
            .snapshot_rows(self.rows);
        let state = ready(log.read(|fold| {
            fold.checkpoint(scope)
                .map_or_else(ScopeState::default, |checkpoint| {
                    ScopeState::restore(&checkpoint.payload)
                })
        }));
        let mut events = Vec::new();
        refused(ready(log.history(0, |event| {
            events.push((Some((0, event.seq)), text(&event.event)));
            true
        })));
        let mut state = state;
        state.events = events;
        self.logs.insert(scope.to_owned(), log);
        state
    }

    fn point(
        &mut self,
        _log: &mut dyn Log,
        scope: &str,
        point: Point,
        state: &mut ScopeState,
    ) -> Option<Appended> {
        let log = self.log(scope);
        let (recovering, floor, revision) = {
            let lease = ready(log.lease());
            let fold = lease.fold();
            (
                fold.recovering(),
                lease.floor_pending(),
                fold.checkpoint(scope)
                    .map_or(0, |checkpoint| checkpoint.revision),
            )
        };
        let (log_point, kind) = self.commit_point(point, recovering, floor)?;
        let log = &self.logs[scope];
        let before = ready(log.stats());
        let mut lease = ready(log.lease());
        if let Some(bytes) = self.state
            && lease.fold().host("state").is_none()
        {
            refused(lease.push(LogRecord::Host {
                key: "state".to_owned(),
                value: raw(&padding(scope, bytes)),
            }));
        }
        let mut checkpoint = state.clone();
        checkpoint.events.clear();
        refused(lease.push(LogRecord::Checkpoint {
            payload: checkpoint.snapshot(),
            revision: revision + 1,
            state: scope.to_owned(),
        }));
        let seq = refused(ready(lease.commit(log_point)));
        if let Some(keep) = self.retention {
            let finished = lease
                .fold()
                .turns()
                .iter()
                .filter(|turn| matches!(turn.phase, Phase::Finished { .. }))
                .count();
            assert!(
                finished <= keep,
                "{finished} finished turns stay in the fold past a retention of {keep}"
            );
        }
        drop(lease);
        self.unsealed.remove(scope);
        let after = ready(log.stats());
        let parts = after.parts - before.parts;
        if let Some(bound) = self.bound {
            let loaded = self.disk.with(|rows| {
                let reads = rows.reads;
                let loaded = rows.tload(scope).len();
                rows.reads = reads;
                loaded
            });
            assert!(
                loaded as u64 <= bound as u64 + parts + 1,
                "a cold open of {scope} reads {loaded} rows past a snapshot row bound of {bound}"
            );
        }
        if self.state.is_some() {
            let snapshots = self.disk.with(|rows| {
                rows.view(scope)
                    .iter()
                    .filter(|(_, body)| body.starts_with('s'))
                    .count()
            });
            assert!(
                snapshots <= 1,
                "{scope} stores {snapshots} snapshot rows, so a superseded snapshot was not archived"
            );
        }
        Some(Appended {
            events: Vec::new(),
            kind,
            parts,
            seq,
            snapshot: after.snapshots > before.snapshots,
        })
    }

    fn record(&mut self, scope: &str, record: Record) {
        for translated in self.records(scope, record) {
            self.push(scope, translated);
        }
    }

    fn restart(&mut self, scope: &str, state: &ScopeState) -> Decision {
        if !self.protections.restart {
            return Decision::Resume(None);
        }
        let request = state
            .active
            .as_ref()
            .expect("a restart decision is asked for an active turn")
            .request
            .clone();
        let log = self.log(scope);
        let attempt = ready(log.read(|fold| fold.next_attempt(&request)));
        if attempt > RESTART_CAP {
            return Decision::Fail;
        }
        self.restarting.insert(scope.to_owned(), request);
        Decision::Resume(Some(Record::Restart { attempt, floor: 0 }))
    }

    fn unsealed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unsealed).into_iter().collect()
    }
}
