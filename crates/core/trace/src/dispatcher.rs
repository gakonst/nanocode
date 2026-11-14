use std::{any::Any, fmt, sync::OnceLock};

use crate::{
    Event, LevelFilter, Metadata, callsite,
    span::{Attributes, Current, Id, Record},
    subscriber::{Interest, NoSubscriber, Subscriber},
};

static GLOBAL: OnceLock<Dispatch> = OnceLock::new();
static NONE: Dispatch = Dispatch::none();
static NO_SUBSCRIBER: NoSubscriber = NoSubscriber::new();

pub struct Dispatch {
    subscriber: Kind,
}

enum Kind {
    Global(&'static (dyn Subscriber + Send + Sync)),
    Owned(Box<dyn Subscriber + Send + Sync>),
}

pub struct SetGlobalDefaultError(());

impl SetGlobalDefaultError {
    const MESSAGE: &'static str = "a global default trace dispatcher has already been set";
}

impl fmt::Debug for SetGlobalDefaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SetGlobalDefaultError")
            .field(&Self::MESSAGE)
            .finish()
    }
}

impl fmt::Display for SetGlobalDefaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(Self::MESSAGE)
    }
}

impl std::error::Error for SetGlobalDefaultError {}

pub fn set_global_default(dispatch: Dispatch) -> Result<(), SetGlobalDefaultError> {
    GLOBAL
        .set(dispatch)
        .map_err(|_| SetGlobalDefaultError(()))?;
    callsite::rebuild_interest_cache();
    Ok(())
}

#[inline]
pub fn get_default<T>(read: impl FnOnce(&'static Dispatch) -> T) -> T {
    read(GLOBAL.get().unwrap_or(&NONE))
}

impl Dispatch {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            subscriber: Kind::Global(&NO_SUBSCRIBER),
        }
    }

    #[must_use]
    pub fn new<S>(subscriber: S) -> Self
    where
        S: Subscriber + Send + Sync,
    {
        Self {
            subscriber: Kind::Owned(Box::new(subscriber)),
        }
    }

    #[inline(always)]
    fn subscriber(&self) -> &(dyn Subscriber + Send + Sync) {
        match &self.subscriber {
            Kind::Global(subscriber) => *subscriber,
            Kind::Owned(subscriber) => subscriber.as_ref(),
        }
    }

    #[inline]
    #[must_use]
    pub fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        self.subscriber().register_callsite(metadata)
    }

    pub(crate) fn max_level_hint(&self) -> Option<LevelFilter> {
        self.subscriber().max_level_hint()
    }

    #[inline]
    #[must_use]
    pub fn new_span(&self, span: &Attributes<'_>) -> Id {
        self.subscriber().new_span(span)
    }

    #[inline]
    pub fn record(&self, span: &Id, values: &Record<'_>) {
        self.subscriber().record(span, values);
    }

    #[inline]
    pub fn record_follows_from(&self, span: &Id, follows: &Id) {
        self.subscriber().record_follows_from(span, follows);
    }

    #[inline]
    #[must_use]
    pub fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.subscriber().enabled(metadata)
    }

    #[inline]
    pub fn event(&self, event: &Event<'_>) {
        let subscriber = self.subscriber();
        if subscriber.event_enabled(event) {
            subscriber.event(event);
        }
    }

    pub fn enter(&self, span: &Id) {
        self.subscriber().enter(span);
    }

    pub fn exit(&self, span: &Id) {
        self.subscriber().exit(span);
    }

    #[inline]
    #[must_use]
    pub fn clone_span(&self, id: &Id) -> Id {
        self.subscriber().clone_span(id)
    }

    pub fn try_close(&self, id: Id) -> bool {
        self.subscriber().try_close(id)
    }

    #[inline]
    #[must_use]
    pub fn current_span(&self) -> Current {
        self.subscriber().current_span()
    }

    #[must_use]
    pub fn is<T: Any>(&self) -> bool {
        self.downcast_ref::<T>().is_some()
    }

    #[must_use]
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        let subscriber: &dyn Any = self.subscriber();
        subscriber.downcast_ref::<T>()
    }
}

impl fmt::Debug for Dispatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.subscriber {
            Kind::Global(subscriber) => formatter
                .debug_tuple("Dispatch::Global")
                .field(&format_args!(
                    "{:p}",
                    std::ptr::from_ref(*subscriber).cast::<()>()
                ))
                .finish(),
            Kind::Owned(subscriber) => formatter
                .debug_tuple("Dispatch::Scoped")
                .field(&format_args!(
                    "{:p}",
                    std::ptr::from_ref(subscriber.as_ref()).cast::<()>()
                ))
                .finish(),
        }
    }
}

impl<S> From<S> for Dispatch
where
    S: Subscriber + Send + Sync,
{
    fn from(subscriber: S) -> Self {
        Self::new(subscriber)
    }
}
