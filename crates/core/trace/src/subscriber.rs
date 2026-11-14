use std::any::Any;

use crate::{
    Event, LevelFilter, Metadata,
    span::{Attributes, Current, Id, Record},
};

pub use crate::dispatcher::SetGlobalDefaultError;

pub trait Subscriber: Any {
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        if self.enabled(metadata) {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool;

    fn max_level_hint(&self) -> Option<LevelFilter> {
        None
    }

    fn new_span(&self, span: &Attributes<'_>) -> Id;

    fn record(&self, span: &Id, values: &Record<'_>);

    fn record_follows_from(&self, span: &Id, follows: &Id);

    fn event_enabled(&self, event: &Event<'_>) -> bool {
        let _ = event;
        true
    }

    fn event(&self, event: &Event<'_>);

    fn enter(&self, span: &Id);

    fn exit(&self, span: &Id);

    fn clone_span(&self, id: &Id) -> Id {
        id.clone()
    }

    fn try_close(&self, id: Id) -> bool {
        let _ = id;
        false
    }

    fn current_span(&self) -> Current {
        Current::unknown()
    }
}

#[derive(Clone, Debug)]
pub struct Interest(InterestKind);

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum InterestKind {
    Never,
    Sometimes,
    Always,
}

impl Interest {
    #[must_use]
    pub const fn never() -> Self {
        Self(InterestKind::Never)
    }

    #[must_use]
    pub const fn sometimes() -> Self {
        Self(InterestKind::Sometimes)
    }

    #[must_use]
    pub const fn always() -> Self {
        Self(InterestKind::Always)
    }

    #[must_use]
    pub const fn is_never(&self) -> bool {
        matches!(self.0, InterestKind::Never)
    }

    #[must_use]
    pub const fn is_sometimes(&self) -> bool {
        matches!(self.0, InterestKind::Sometimes)
    }

    #[must_use]
    pub const fn is_always(&self) -> bool {
        matches!(self.0, InterestKind::Always)
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct NoSubscriber(());

impl NoSubscriber {
    #[must_use]
    pub const fn new() -> Self {
        Self(())
    }
}

const NO_SUBSCRIBER_SPAN: u64 = 0xDEAD;

impl Subscriber for NoSubscriber {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::never()
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(NO_SUBSCRIBER_SPAN)
    }

    fn event(&self, _: &Event<'_>) {}

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn enabled(&self, _: &Metadata<'_>) -> bool {
        false
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

pub fn set_global_default<S>(subscriber: S) -> Result<(), SetGlobalDefaultError>
where
    S: Subscriber + Send + Sync,
{
    crate::dispatcher::set_global_default(crate::Dispatch::new(subscriber))
}
