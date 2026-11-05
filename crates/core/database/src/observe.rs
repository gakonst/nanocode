use std::{fmt::Display, time::Duration};

pub trait Observe: Send + Sync + 'static {
    fn wakes_overflowed(&self) {}

    fn wait_unwatched(&self) {}

    fn session_lock_state(&self, _kind: &'static str, _state: &'static str, _change: f64) {}

    fn session_lock_late(&self, _kind: &'static str, _waited: Duration) {}

    fn session_lock_waited(&self, _kind: &'static str, _waited: Duration) {}

    fn session_lock_starved(&self, _kind: &'static str) {}

    fn session_lock_lost(&self, _kind: &'static str, _error: &dyn Display) {}

    fn session_lock_unreleased(&self, _kind: &'static str, _error: &dyn Display) {}

    fn pipeline_failed(&self, _error: &dyn Display) {}

    fn listen_failed(&self, _error: &dyn Display) {}

    fn forward_unavailable(&self, _error: &dyn Display) {}

    fn forward_failed(&self, _error: &dyn Display) {}
}

impl Observe for () {}
