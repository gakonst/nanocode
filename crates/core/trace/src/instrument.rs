use nanocode_sys::{Scope, Scoped};

use crate::span::{Entered, Span};

pub type Instrumented<T> = Scoped<T, Span>;

pub trait Instrument: Sized {
    fn instrument(self, span: Span) -> Instrumented<Self> {
        Scoped::new(self, span)
    }

    fn in_current_span(self) -> Instrumented<Self> {
        self.instrument(Span::current())
    }
}

impl<T: Sized> Instrument for T {}

impl Scope for Span {
    type Entered<'a> = Entered<'a>;

    fn enter(&self) -> Entered<'_> {
        Self::enter(self)
    }
}
