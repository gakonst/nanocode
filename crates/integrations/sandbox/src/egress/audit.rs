use crate::egress::{
    authority::{Authority, chain},
    lookup::Key,
    wire::{Called, Calls, Credential, Report},
};
use nanocode_platform::{MissedTickBehavior, interval_at};
use nanocode_sync::mpsc;
use nanocode_trace::{error, warn};
use std::{
    collections::{HashMap, VecDeque},
    mem,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy)]
pub struct Reporting {
    pub every: Duration,
    pub batch: usize,
    pub hosts: usize,
    pub refused_hosts: usize,
    pub other: &'static str,
    pub queue: usize,
    pub pending: usize,
    pub kept: Duration,
}

type Held = HashMap<Key, HashMap<Entry, Tally>>;
type Pending = VecDeque<(i64, Report)>;

#[derive(Clone, Copy, Default)]
struct Tally {
    requests: i64,
    at: i64,
}

pub struct Audit {
    counts: mpsc::Sender<Count>,
    lost: Arc<AtomicU64>,
}

impl Audit {
    pub fn start<A: Authority>(authority: Arc<A>, reporting: Reporting) -> Self {
        let (counts, received) = mpsc::channel(reporting.queue);
        let lost = Arc::new(AtomicU64::new(0));
        nanocode_platform::spawn_task(own(authority, reporting, received, Arc::clone(&lost)));
        Self { counts, lost }
    }

    pub fn count(
        &self,
        credential: &Key,
        host: &str,
        port: u16,
        kind: Kind,
        pattern: Option<String>,
    ) {
        let entry = Entry {
            host: host.chars().take(253).collect(),
            port,
            kind,
        };
        let sent = self.counts.try_send(Count {
            credential: Arc::clone(credential),
            entry,
            pattern,
        });
        if sent.is_err() {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Intercepted,
    Tunneled,
    Refused,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Entry {
    host: String,
    port: u16,
    kind: Kind,
}

struct Count {
    credential: Key,
    entry: Entry,
    pattern: Option<String>,
}

struct Reported {
    key: Key,
    entry: Entry,
    tally: Tally,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

async fn own<A: Authority>(
    authority: Arc<A>,
    reporting: Reporting,
    mut counts: mpsc::Receiver<Count>,
    lost: Arc<AtomicU64>,
) {
    let mut held = Held::new();
    let mut pending = Pending::new();
    let mut tick = interval_at(nanocode_platform::now() + reporting.every, reporting.every);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        nanocode_sync::select! {
            _ = tick.tick() => {
                let lost = lost.swap(0, Ordering::Relaxed);
                if lost > 0 {
                    error!("egress audit queue full: {lost} counts lost since the last report");
                }
                report(&*authority, &reporting, &mut held, &mut pending).await;
            }
            count = counts.recv() => match count {
                Some(count) => add(&reporting, &mut held, count),
                None => return,
            },
        }
    }
}

fn add(
    reporting: &Reporting,
    held: &mut Held,
    Count {
        credential,
        mut entry,
        pattern,
    }: Count,
) {
    let hand = held.entry(credential).or_default();
    if !hand.contains_key(&entry) {
        let cap = if entry.kind == Kind::Refused {
            reporting.refused_hosts
        } else {
            reporting.hosts
        };
        let same = hand
            .keys()
            .filter(|seen| {
                seen.kind == entry.kind && seen.host != reporting.other && seen.port != 0
            })
            .count();
        if same >= cap {
            entry.host = pattern.unwrap_or_else(|| reporting.other.into());
            entry.port = 0;
        }
    }
    let tally = hand.entry(entry).or_default();
    tally.requests += 1;
    tally.at = now_secs();
}

async fn report<A: Authority>(
    authority: &A,
    reporting: &Reporting,
    held: &mut Held,
    pending: &mut Pending,
) {
    let now = now_secs();
    let entries: Vec<Reported> = mem::take(held)
        .into_iter()
        .flat_map(|(key, called)| {
            called.into_iter().map(move |(entry, tally)| Reported {
                key: Arc::clone(&key),
                entry,
                tally,
            })
        })
        .collect();
    for batch in entries.chunks(reporting.batch) {
        let mut callers: Vec<Calls> = Vec::new();
        for reported in batch {
            let (key, entry) = (&reported.key, &reported.entry);
            let called = Called {
                host: entry.host.clone(),
                intercepted: entry.kind == Kind::Intercepted,
                port: i32::from(entry.port),
                refused: entry.kind == Kind::Refused,
                requests: reported.tally.requests,
                at: Some(reported.tally.at),
            };
            match callers.iter_mut().find(|calls| calls.caller == **key) {
                Some(calls) => calls.called.push(called),
                None => callers.push(Calls {
                    called: vec![called],
                    caller: Credential::clone(key),
                }),
            }
        }
        let id = nanocode_platform::uuid_v4().simple().to_string();
        pending.push_back((now, Report { id, callers }));
    }
    let kept = i64::try_from(reporting.kept.as_secs()).unwrap_or(i64::MAX);
    let mut dropped = 0;
    while pending.front().is_some_and(|(made, _)| {
        pending.len() > reporting.pending || *made < now.saturating_sub(kept)
    }) {
        pending.pop_front();
        dropped += 1;
    }
    if dropped > 0 {
        error!("egress reports dropped unsent: {dropped}, the backend didn't take them in time");
    }
    while let Some((_, report)) = pending.front() {
        if let Err(error) = authority.report(report).await {
            warn!(
                "egress report not sent, kept for the next: {}",
                chain(&*error)
            );
            return;
        }
        pending.pop_front();
    }
}
