use std::time::Duration;

use nanocode_tool_host::embedded::HostFuture;
use serde_json::{Map, Value};

use crate::{
    SessionEvent,
    durability::StoreError,
    session_log::{LoggedEvent, Started, TurnRecord, TurnState},
};

pub type Logged<'a, T> = HostFuture<'a, Result<T, StoreError>>;

pub struct Recording<'a> {
    pub session_id: &'a str,
    pub annotation: &'a Map<String, Value>,
    pub events: &'a [SessionEvent],
    pub request_ids: &'a [Option<String>],
    pub read: &'a (dyn Fn(&str) -> bool + Send + Sync),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Verdict<T> {
    Accepted,
    Existing(T),
    Conflict(T),
    Steered(T),
}

impl<T> Verdict<T> {
    pub fn of(previous: Option<(T, bool)>, active: Option<T>, steers: bool) -> Self {
        match (previous, active) {
            (Some((previous, true)), _) => Self::Existing(previous),
            (Some((previous, false)), _) => Self::Conflict(previous),
            (None, Some(active)) if steers => Self::Steered(active),
            (None, Some(active)) => Self::Conflict(active),
            (None, None) => Self::Accepted,
        }
    }
}

pub trait EventLog: Send + Sync + 'static {
    fn record<'a>(
        &'a self,
        recording: Recording<'a>,
    ) -> HostFuture<'a, Result<Vec<LoggedEvent>, String>>;

    fn flush(&self) -> Logged<'_, ()>;

    fn tick(&self) -> Option<Duration> {
        None
    }
}

pub trait TurnLog: EventLog {
    fn turn<'a>(&'a self, request_id: &'a str) -> Logged<'a, Option<TurnRecord>>;

    fn pending(&self) -> Logged<'_, Vec<TurnRecord>>;

    fn start<'a>(&'a self, request_id: &'a str) -> Logged<'a, Option<Started>>;

    fn finish<'a>(
        &'a self,
        request_id: &'a str,
        state: TurnState,
        fields: &'a Map<String, Value>,
    ) -> Logged<'a, Option<(TurnRecord, LoggedEvent)>>;

    fn cancel<'a>(
        &'a self,
        request_id: &'a str,
        fields: &'a Map<String, Value>,
    ) -> Logged<'a, Option<(TurnRecord, Option<LoggedEvent>)>>;

    fn withdraw_steered<'a>(
        &'a self,
        request_id: &'a str,
        digest: &'a str,
    ) -> Logged<'a, Option<(TurnRecord, LoggedEvent)>>;
}
