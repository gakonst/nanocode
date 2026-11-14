use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};

use nanocode_agent_core::backend::{BackendFuture, ParkedCall};
use nanocode_sync::Notify;
use nanocode_sync::{
    Atom, Signal, Slot, StreamExt,
    future::{AbortHandle, Abortable},
};
use nanocode_trace::Instrument as _;

use crate::{ClaudeClient, ClaudeError, ClaudeStream, PreparedWire, StreamEvent};

const OPENING: u8 = 0;
const STREAMING: u8 = 1;
const REFUSED: u8 = 2;
const ENDED: u8 = 3;
const FAILED: u8 = 4;

#[derive(PartialEq, Eq)]
pub(super) struct Key {
    pub(super) operation: String,
    pub(super) step: String,
    pub(super) request: Box<str>,
}

pub(super) struct ModelCall {
    key: Key,
    events: Atom<nanocode_sync::Vector<StreamEvent>>,
    phase: AtomicU8,
    failure: Slot<ClaudeError>,
    changed: Notify,
    done: Signal,
    abort: AbortHandle,
}

impl ModelCall {
    pub(super) fn start(
        client: ClaudeClient,
        wire: PreparedWire,
        key: Key,
        span: nanocode_trace::Span,
    ) -> Arc<Self> {
        let (abort, registration) = AbortHandle::new_pair();
        let call = Arc::new(Self {
            key,
            events: Atom::default(),
            phase: AtomicU8::new(OPENING),
            failure: Slot::default(),
            changed: Notify::new(),
            done: Signal::new(),
            abort,
        });
        let running = Arc::clone(&call);
        let receiving = async move {
            let opened = Box::pin(client.stream_prepared(&wire)).await;
            drop((client, wire));
            let mut stream = match opened {
                Ok(stream) => stream,
                Err(error) => return running.end(REFUSED, Some(error)),
            };
            running.phase.store(STREAMING, Ordering::SeqCst);
            running.changed.notify_waiters();
            loop {
                match stream.next().await {
                    Some(Ok(event)) => {
                        running
                            .events
                            .update(|events| events.push_back(event.clone()));
                        running.changed.notify_waiters();
                    }
                    Some(Err(error)) => return running.end(FAILED, Some(error)),
                    None => return running.end(ENDED, None),
                }
            }
        };
        super::spawn_task(Box::pin(async move {
            let _ = Abortable::new(receiving.instrument(span), registration).await;
        }));
        call
    }

    fn end(&self, phase: u8, error: Option<ClaudeError>) {
        if let Some(error) = error {
            self.failure.put(error);
        }
        self.phase.store(phase, Ordering::SeqCst);
        self.changed.notify_waiters();
        self.done.fire();
    }

    pub(super) async fn opened(&self) -> Result<(), ClaudeError> {
        loop {
            let changed = self.changed.notified();
            let mut changed = std::pin::pin!(changed);
            changed.as_mut().enable();
            match self.phase.load(Ordering::SeqCst) {
                OPENING => changed.await,
                REFUSED => {
                    return Err(self.failure.take().unwrap_or(ClaudeError::IncompleteStream));
                }
                _ => return Ok(()),
            }
        }
    }

    async fn next(&self, read: usize) -> Option<Result<StreamEvent, ClaudeError>> {
        loop {
            let changed = self.changed.notified();
            let mut changed = std::pin::pin!(changed);
            changed.as_mut().enable();
            let phase = self.phase.load(Ordering::SeqCst);
            if let Some(event) = self.events.read(|events| events.get(read).cloned()) {
                return Some(Ok(event));
            }
            match phase {
                ENDED => return None,
                FAILED => {
                    return Some(Err(self
                        .failure
                        .take()
                        .unwrap_or(ClaudeError::IncompleteStream)));
                }
                _ => changed.await,
            }
        }
    }

    pub(super) fn stream(self: &Arc<Self>) -> ClaudeStream {
        Box::pin(nanocode_sync::stream::unfold(
            (Arc::clone(self), 0_usize),
            |(call, read)| async move {
                let next = call.next(read).await?;
                let read = read + usize::from(next.is_ok());
                Some((next, (call, read)))
            },
        ))
    }
}

pub(super) struct Flight(pub(super) Arc<ModelCall>);

impl Drop for Flight {
    fn drop(&mut self) {
        self.0.abort.abort();
    }
}

impl ParkedCall for Flight {
    fn finished(&self) -> BackendFuture<()> {
        let call = Arc::clone(&self.0);
        Box::pin(async move { call.done.wait().await })
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync> {
        self
    }
}

const RUNNING: u8 = 0;
const PARKED: u8 = 1;
const CLAIMED: u8 = 2;

#[derive(Default)]
pub(super) struct Parking {
    enabled: AtomicBool,
    state: AtomicU8,
    entries: AtomicU64,
    entered: Notify,
    flight: Slot<Arc<Flight>>,
    adopted: Slot<Arc<Flight>>,
}

pub(super) struct Parked<'a>(&'a Parking);

impl Parked<'_> {
    pub(super) fn leave(self) -> bool {
        self.0.leave()
    }
}

impl Drop for Parked<'_> {
    fn drop(&mut self) {
        self.0.leave();
    }
}

impl Parking {
    pub(super) fn set(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub(super) fn enter(
        &self,
        key: Key,
        start: impl FnOnce(Key) -> Arc<ModelCall>,
    ) -> Option<(Arc<ModelCall>, Parked<'_>)> {
        let flight = match self.adopted.take() {
            Some(flight) if flight.0.key == key => flight,
            Some(_) => {
                self.set(false);
                return None;
            }
            None => Arc::new(Flight(start(key))),
        };
        let call = Arc::clone(&flight.0);
        self.flight.put(flight);
        self.state.store(PARKED, Ordering::SeqCst);
        self.entries.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_waiters();
        Some((call, Parked(self)))
    }

    fn leave(&self) -> bool {
        if self
            .state
            .compare_exchange(PARKED, RUNNING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            drop(self.flight.take());
        }
        self.state.load(Ordering::SeqCst) != CLAIMED
    }

    pub(super) fn claim(&self) -> Option<Arc<Flight>> {
        self.state
            .compare_exchange(PARKED, CLAIMED, Ordering::SeqCst, Ordering::SeqCst)
            .ok()?;
        self.flight.take()
    }

    pub(super) fn adopt(&self, call: Arc<dyn ParkedCall>) {
        if let Ok(flight) = call.into_any().downcast::<Flight>() {
            self.adopted.put(flight);
            self.set(true);
        }
    }

    pub(super) async fn parked(&self, after: u64) -> u64 {
        loop {
            let entered = self.entered.notified();
            let mut entered = std::pin::pin!(entered);
            entered.as_mut().enable();
            let entries = self.entries.load(Ordering::SeqCst);
            if entries > after && self.state.load(Ordering::SeqCst) == PARKED {
                return entries;
            }
            entered.await;
        }
    }
}
