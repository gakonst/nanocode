pub mod callsite;
pub mod dispatcher;
mod event;
pub mod field;
mod instrument;
pub mod level_filters;
mod macros;
pub mod metadata;
pub mod span;
pub mod subscriber;

pub use callsite::Callsite;
pub use dispatcher::Dispatch;
pub use event::Event;
pub use field::Value;
pub use instrument::{Instrument, Instrumented};
pub use metadata::{Level, LevelFilter, Metadata};
pub use span::Span;
pub use subscriber::{Interest, Subscriber};

#[doc(hidden)]
pub mod __private {
    use crate::{Metadata, dispatcher, subscriber::Interest};

    pub fn is_enabled(metadata: &Metadata<'static>, interest: &Interest) -> bool {
        interest.is_always() || dispatcher::get_default(|current| current.enabled(metadata))
    }
}
