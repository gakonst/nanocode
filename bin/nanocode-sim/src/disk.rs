use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use nanocode_sync::Slot;

#[derive(Default)]
pub struct Rows {
    pub appends: Vec<(u64, String, String)>,
    pub log: BTreeMap<u64, (String, String)>,
    pub reads: u64,
    pub snaps: BTreeMap<String, (u64, String)>,
    pub staged: Vec<(String, u64, String)>,
}

fn count(len: usize) -> u64 {
    u64::try_from(len).expect("a row count fits in u64")
}

impl Rows {
    pub fn append(&mut self, scope: &str, body: String) -> u64 {
        let committed = self.log.keys().next_back().copied().unwrap_or(0);
        let pending = self.appends.last().map_or(0, |(seq, ..)| *seq);
        let seq = committed.max(pending) + 1;
        self.appends.push((seq, scope.to_owned(), body));
        seq
    }

    pub fn snapshot(&mut self, scope: &str) -> Option<(u64, String)> {
        let snap = self
            .staged
            .iter()
            .rev()
            .find(|(owner, ..)| owner == scope)
            .map(|(_, seq, body)| (*seq, body.clone()))
            .or_else(|| self.snaps.get(scope).cloned());
        self.reads += u64::from(snap.is_some());
        snap
    }

    pub fn scan(&mut self, scope: &str, after: u64) -> Vec<(u64, String)> {
        let rows: Vec<(u64, String)> = self
            .log
            .range(after + 1..)
            .map(|(seq, (owner, body))| (*seq, owner, body))
            .chain(
                self.appends
                    .iter()
                    .map(|(seq, owner, body)| (*seq, owner, body)),
            )
            .filter(|(seq, owner, _)| *seq > after && owner.as_str() == scope)
            .map(|(seq, _, body)| (seq, body.clone()))
            .collect();
        self.reads += count(rows.len());
        rows
    }

    pub fn stage(&mut self, scope: &str, seq: u64, body: String) {
        self.staged.push((scope.to_owned(), seq, body));
    }

    pub fn scopes(&self) -> BTreeSet<String> {
        self.log
            .values()
            .map(|(scope, _)| scope.clone())
            .chain(self.snaps.keys().cloned())
            .collect()
    }
}

pub struct Disk(Slot<Rows>);

impl Disk {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Slot::new(Some(Rows::default()))))
    }

    pub fn with<R>(&self, change: impl FnOnce(&mut Rows) -> R) -> R {
        let mut rows = self
            .0
            .take()
            .expect("the simulator runs one tick at a time, so its disk is never taken twice");
        let result = change(&mut rows);
        assert!(
            self.0.put(rows).is_none(),
            "the simulated disk has one home"
        );
        result
    }
}
