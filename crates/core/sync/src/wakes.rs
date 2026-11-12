use crate::Atom;
use std::{
    collections::{HashMap, HashSet},
    hash::{BuildHasher, RandomState},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::{Notify, broadcast, mpsc};

pub type Wakeups = broadcast::Receiver<Arc<str>>;

type Hook = Arc<dyn Fn(Option<&'static str>, &str) + Send + Sync>;

type Waiting = crate::Map<Arc<str>, crate::OrdMap<u64, Arc<Notify>>>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Wake {
    pub channel: String,
    pub payload: String,
    pub collected: bool,
}

impl Wake {
    #[must_use]
    pub fn new(channel: &str, payload: &str, collected: bool) -> Self {
        Self {
            channel: channel.to_owned(),
            payload: payload.to_owned(),
            collected,
        }
    }
}

pub struct Wakeup {
    hub: WakeHub,
    lane: &'static str,
    key: Arc<str>,
    id: u64,
    notify: Arc<Notify>,
}

impl Wakeup {
    pub async fn changed(&self) {
        self.notify.notified().await;
    }
}

impl Drop for Wakeup {
    fn drop(&mut self) {
        if let Some(Lane { keyed, .. }) = self.hub.0.lanes.get(self.lane) {
            keyed.shard(&self.key).update(|waiting| {
                if let Some(list) = waiting.get_mut(&self.key) {
                    list.remove(&self.id);
                    if list.is_empty() {
                        waiting.remove(&self.key);
                    }
                }
            });
        }
    }
}

#[derive(Clone)]
pub struct WakeHub(Arc<Inner>);

impl WakeHub {
    #[must_use]
    pub fn new(
        channels: impl IntoIterator<Item = &'static str>,
        lane: usize,
        shards: usize,
    ) -> Self {
        let lanes = channels
            .into_iter()
            .map(|name| {
                let keyed = Keyed {
                    shards: (0..shards.max(1)).map(|_| Atom::default()).collect(),
                    hasher: RandomState::new(),
                };
                let sender = broadcast::channel(lane).0;
                (name, Lane { sender, keyed })
            })
            .collect();
        Self(Arc::new(Inner {
            lanes,
            next: AtomicU64::new(0),
            hooks: Atom::default(),
        }))
    }

    #[must_use]
    pub fn named(&self, channel: &str) -> Option<&'static str> {
        self.0.lanes.get_key_value(channel).map(|(name, _)| *name)
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.lanes.keys().copied()
    }

    #[must_use]
    pub fn subscribe(&self, channel: &str) -> Option<Wakeups> {
        Some(self.0.lanes.get(channel)?.sender.subscribe())
    }

    #[must_use]
    pub fn keyed(&self, channel: &str, key: &str) -> Option<Wakeup> {
        let (lane, Lane { keyed, .. }) = self.0.lanes.get_key_value(channel)?;
        let key: Arc<str> = key.into();
        let id = self.0.next.fetch_add(1, Ordering::Relaxed);
        let notify = Arc::new(Notify::new());
        keyed.shard(&key).update(|waiting| {
            waiting
                .entry(Arc::clone(&key))
                .or_default()
                .insert(id, Arc::clone(&notify));
        });
        Some(Wakeup {
            hub: self.clone(),
            lane,
            key,
            id,
            notify,
        })
    }

    pub fn hook(&self, hook: impl Fn(Option<&'static str>, &str) + Send + Sync + 'static) {
        let hook: Hook = Arc::new(hook);
        self.0.hooks.update(|hooks| hooks.push(Arc::clone(&hook)));
    }

    pub fn wake(&self, channel: Option<&str>, id: &Arc<str>) {
        let hooks = self.0.hooks.get();
        let named = match channel {
            None => Some(None),
            Some(name) => self.named(name).map(Some),
        };
        if let Some(named) = named {
            hooks.iter().for_each(|hook| hook(named, id));
        }
        for (name, Lane { sender, keyed }) in &self.0.lanes {
            if channel.is_none_or(|channel| channel == *name) {
                drop(sender.send(Arc::clone(id)));
                keyed.wake(channel.map(|_| &**id).filter(|id| !id.is_empty()));
            }
        }
    }
}

pub struct WakeOutbox {
    wakes: mpsc::Receiver<Wake>,
    overflow: Arc<AtomicBool>,
}

impl WakeOutbox {
    pub async fn recv_many(&mut self, batch: &mut Vec<Wake>, limit: usize) -> usize {
        self.wakes.recv_many(batch, limit).await
    }

    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.overflow.swap(false, Ordering::Relaxed)
    }
}

pub struct Wakes {
    hub: WakeHub,
    others: mpsc::Sender<Wake>,
    overflow: Arc<AtomicBool>,
    overflowed: fn(),
}

impl Wakes {
    #[must_use]
    pub fn new(hub: WakeHub, queue: usize, overflowed: fn()) -> (Self, WakeOutbox) {
        let (others, wakes) = mpsc::channel(queue);
        let overflow = Arc::new(AtomicBool::new(false));
        let outbox = WakeOutbox {
            wakes,
            overflow: Arc::clone(&overflow),
        };
        (
            Self {
                hub,
                others,
                overflow,
                overflowed,
            },
            outbox,
        )
    }

    pub fn deliver<'a>(&self, wakes: impl IntoIterator<Item = (&'a str, &'a str, bool)>) {
        let mut seen = HashSet::new();
        for (channel, payload, collected) in wakes {
            if seen.insert((channel, payload)) {
                self.hub.wake(Some(channel), &Arc::from(payload));
                if self
                    .others
                    .try_send(Wake::new(channel, payload, collected))
                    .is_err()
                {
                    (self.overflowed)();
                    self.overflow.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}

struct Keyed {
    shards: Box<[Atom<Waiting>]>,
    hasher: RandomState,
}

impl Keyed {
    fn shard(&self, key: &str) -> &Atom<Waiting> {
        let count = self.shards.len() as u64;
        let index = usize::try_from(self.hasher.hash_one(key) % count).unwrap_or(0);
        &self.shards[index % self.shards.len()]
    }

    fn wake(&self, key: Option<&str>) {
        let notify = |waiting: &Waiting| match key {
            Some(key) => waiting
                .get(key)
                .into_iter()
                .flat_map(crate::OrdMap::values)
                .for_each(|notify| notify.notify_one()),
            None => waiting
                .values()
                .flat_map(crate::OrdMap::values)
                .for_each(|notify| notify.notify_one()),
        };
        match key {
            Some(key) => self.shard(key).read(notify),
            None => self.shards.iter().for_each(|shard| shard.read(notify)),
        }
    }
}

struct Lane {
    sender: broadcast::Sender<Arc<str>>,
    keyed: Keyed,
}

struct Inner {
    lanes: HashMap<&'static str, Lane>,
    next: AtomicU64,
    hooks: Atom<Vec<Hook>>,
}

pub async fn woken(wakeups: &mut Wakeups, key: &str) {
    loop {
        match wakeups.recv().await {
            Ok(id) if id.is_empty() || *id == *key => return,
            Ok(_) => {}
            Err(_) => return,
        }
    }
}
