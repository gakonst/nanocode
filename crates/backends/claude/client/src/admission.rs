use std::{sync::Arc, time::Duration};

use nanocode_platform::Instant;
use nanocode_sync::{Atom, Notify};

#[derive(Default)]
pub(crate) struct Admission {
    released: Notify,
    state: Atom<State>,
}

#[derive(Clone, Copy, Default)]
struct State {
    epoch: u64,
    in_flight: usize,
    resume: Option<Instant>,
    window: Option<f64>,
}

enum Closed {
    Cooling(Instant),
    Full,
}

pub(crate) struct Permit {
    admission: Arc<Admission>,
    epoch: u64,
}

impl Admission {
    pub(crate) async fn admit(self: &Arc<Self>) -> Permit {
        loop {
            let now = nanocode_platform::now();
            let cooling = {
                let mut released = std::pin::pin!(self.released.notified());
                released.as_mut().enable();
                let admitted = self.state.modify(|state| {
                    if let Some(resume) = state.resume.filter(|resume| *resume > now) {
                        return Err(Closed::Cooling(resume));
                    }
                    if state
                        .window
                        .is_some_and(|window| state.in_flight as f64 >= window)
                    {
                        return Err(Closed::Full);
                    }
                    state.in_flight += 1;
                    Ok(state.epoch)
                });
                match admitted {
                    Ok(epoch) => {
                        return Permit {
                            admission: Arc::clone(self),
                            epoch,
                        };
                    }
                    Err(Closed::Cooling(resume)) => Some(resume),
                    Err(Closed::Full) => {
                        released.await;
                        None
                    }
                }
            };
            if let Some(resume) = cooling {
                nanocode_platform::sleep(resume.saturating_duration_since(now)).await;
            }
        }
    }
}

impl Permit {
    pub(crate) fn accepted(self) {
        let opened = self.admission.state.update(|state| match state.window {
            Some(window) if state.in_flight as f64 >= window.floor() => {
                let grown = window + 1.0 / window;
                state.window = Some(grown);
                grown.floor() > window.floor()
            }
            _ => false,
        });
        if opened {
            self.admission.released.notify_one();
        }
    }

    pub(crate) fn throttled(self, retry_after: Option<Duration>) {
        let now = nanocode_platform::now();
        self.admission.state.update(|state| {
            if state.epoch == self.epoch {
                state.epoch += 1;
                let halved = (state.in_flight as f64 / 2.0).max(1.0);
                state.window = Some(halved);
            }
            if let Some(resume) = retry_after.and_then(|delay| now.checked_add(delay)) {
                state.resume = Some(state.resume.map_or(resume, |current| current.max(resume)));
            }
        });
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.admission.state.update(|state| state.in_flight -= 1);
        self.admission.released.notify_one();
    }
}
