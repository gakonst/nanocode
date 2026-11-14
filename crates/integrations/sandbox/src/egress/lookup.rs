use crate::egress::{
    authority::{Authority, chain},
    wire::{Credential, Grant, Secret},
};
use nanocode_platform::spawn_task as spawn;
use nanocode_sync::{Atom, Signal};
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::time::sleep;
use tracing::error;

#[derive(Clone, Copy)]
pub struct Caching {
    pub granted: Duration,
    pub refused: Duration,
    pub recheck: Duration,
    pub gather: Duration,
    pub most: usize,
    pub flight: Duration,
}

pub type Key = Arc<Credential>;

type Flights = Arc<Atom<nanocode_sync::Map<Key, Arc<Flight>>>>;

pub struct Client {
    pub credential: Key,
    pub grant: Grant,
}

impl Client {
    #[must_use]
    pub fn pattern(&self, host: &str) -> Option<String> {
        let mut bound = self
            .grant
            .secrets
            .iter()
            .filter_map(|secret| secret.binds(host));
        bound.next().map(str::to_owned)
    }

    #[must_use]
    pub fn bound(&self, host: &str) -> Vec<&Secret> {
        self.grant
            .secrets
            .iter()
            .filter(|secret| secret.injects(host))
            .collect()
    }

    #[must_use]
    pub fn set(&self, host: &str) -> Vec<&Secret> {
        self.grant
            .secrets
            .iter()
            .filter(|secret| secret.sets(host))
            .take(1)
            .collect()
    }
}

#[derive(Clone)]
pub enum Verdict {
    Granted(Arc<Client>),
    Refused,
    Unreachable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Route,
    Recheck,
    Inject,
}

pub struct Lookups<A> {
    caching: Caching,
    asker: Arc<Asker<A>>,
    known: Atom<nanocode_sync::Map<Key, Answer>>,
    flights: Flights,
}

impl<A: Authority> Lookups<A> {
    pub fn new(authority: Arc<A>, client: &'static str, caching: Caching) -> Self {
        Self {
            caching,
            asker: Arc::new(Asker {
                authority,
                client,
                generation: AtomicU64::new(1),
            }),
            known: Atom::default(),
            flights: Arc::default(),
        }
    }

    pub async fn resolve(&self, key: &Key, purpose: Purpose) -> Verdict {
        if !self.asker.authority.cached() {
            return self.asker.ask(key).await.verdict;
        }
        if let Some(verdict) = self.cached(key, purpose) {
            return verdict;
        }
        let Answer { asked, verdict } = match purpose {
            Purpose::Inject => self.shared(key).await,
            Purpose::Route | Purpose::Recheck => self.asker.ask(key).await,
        };
        if !matches!(verdict, Verdict::Unreachable) {
            self.known.update(|known| {
                if known.len() > self.caching.most {
                    known.retain(|_, held| held.asked.at.elapsed() < self.caching.granted);
                }
                if known.len() > self.caching.most {
                    known.clear();
                }
                if known
                    .get(key)
                    .is_none_or(|held| held.asked.generation < asked.generation)
                {
                    known.insert(
                        Arc::clone(key),
                        Answer {
                            asked,
                            verdict: verdict.clone(),
                        },
                    );
                }
            });
        }
        verdict
    }

    fn cached(&self, key: &Key, purpose: Purpose) -> Option<Verdict> {
        let keep = match purpose {
            Purpose::Inject => return None,
            Purpose::Recheck => self.caching.recheck,
            Purpose::Route => self.caching.granted,
        };
        self.known.read(|known| {
            let Answer { asked, verdict } = known.get(key)?;
            let keep = if matches!(verdict, Verdict::Granted(_)) {
                keep
            } else {
                keep.min(self.caching.refused)
            };
            (asked.at.elapsed() < keep).then(|| verdict.clone())
        })
    }

    async fn shared(&self, key: &Key) -> Answer {
        let flight = self.flights.update(|flights| {
            if let Some(gathering) = flights
                .get(key)
                .filter(|flight| !flight.asking.load(Ordering::Acquire))
            {
                return Arc::clone(gathering);
            }
            flights.retain(|_, flight| flight.at.elapsed() < self.caching.flight);
            let flight = Arc::new(Flight {
                at: Instant::now(),
                asking: AtomicBool::new(false),
                started: AtomicBool::new(false),
                answer: OnceLock::new(),
                answered: Signal::new(),
            });
            flights.insert(Arc::clone(key), Arc::clone(&flight));
            flight
        });
        if !flight.started.swap(true, Ordering::AcqRel) {
            let (state, asker, all, asking, gather) = (
                Arc::clone(&flight),
                Arc::clone(&self.asker),
                Arc::clone(&self.flights),
                Arc::clone(key),
                self.caching.gather,
            );
            spawn(async move {
                sleep(gather).await;
                state.asking.store(true, Ordering::Release);
                let answer = asker.ask(&asking).await;
                drop(state.answer.set(answer));
                state.answered.fire();
                all.update(|flights| {
                    if flights
                        .get(&asking)
                        .is_some_and(|flight| Arc::ptr_eq(flight, &state))
                    {
                        flights.remove(&asking);
                    }
                });
            });
        }
        flight.answered.wait().await;
        flight.answer.get().cloned().unwrap_or(Answer {
            asked: Asked {
                generation: 0,
                at: Instant::now(),
            },
            verdict: Verdict::Unreachable,
        })
    }
}

#[derive(Clone, Copy)]
struct Asked {
    generation: u64,
    at: Instant,
}

#[derive(Clone)]
struct Answer {
    asked: Asked,
    verdict: Verdict,
}

struct Flight {
    at: Instant,
    asking: AtomicBool,
    started: AtomicBool,
    answer: OnceLock<Answer>,
    answered: Signal,
}

struct Asker<A> {
    authority: Arc<A>,
    client: &'static str,
    generation: AtomicU64,
}

impl<A: Authority> Asker<A> {
    async fn ask(&self, key: &Key) -> Answer {
        let asked = Asked {
            generation: self.generation.fetch_add(1, Ordering::Relaxed),
            at: Instant::now(),
        };
        let verdict = match self.authority.grant(key).await {
            Ok(Some(grant)) => Verdict::Granted(Arc::new(Client {
                credential: Arc::clone(key),
                grant,
            })),
            Ok(None) => Verdict::Refused,
            Err(error) => {
                let (client, name) = (self.client, &key.name);
                let error = chain(&*error);
                error!(%name, "resolving {client} {name} failed: {error}");
                Verdict::Unreachable
            }
        };
        Answer { asked, verdict }
    }
}
