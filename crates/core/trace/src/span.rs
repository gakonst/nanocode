use std::{
    cmp, fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem,
    num::NonZeroU64,
    ops::Deref,
};

use crate::{
    Metadata,
    dispatcher::{self, Dispatch},
    field::{self, AsField, FieldSet},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Id(NonZeroU64);

#[derive(Debug)]
pub(crate) enum Parent {
    Root,
    Current,
    Explicit(Id),
}

#[derive(Debug)]
pub struct Attributes<'a> {
    metadata: &'static Metadata<'static>,
    values: &'a field::ValueSet<'a>,
    parent: Parent,
}

#[derive(Debug)]
pub struct Record<'a> {
    values: &'a field::ValueSet<'a>,
}

#[derive(Debug)]
pub struct Current {
    inner: CurrentInner,
}

#[derive(Debug)]
enum CurrentInner {
    Current {
        id: Id,
        metadata: &'static Metadata<'static>,
    },
    None,
    Unknown,
}

impl Id {
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(NonZeroU64::new(value).expect("span IDs must be > 0"))
    }

    #[must_use]
    pub const fn from_non_zero_u64(id: NonZeroU64) -> Self {
        Self(id)
    }

    #[allow(clippy::wrong_self_convention)]
    #[must_use]
    pub const fn into_u64(&self) -> u64 {
        self.0.get()
    }

    #[allow(clippy::wrong_self_convention)]
    #[must_use]
    pub const fn into_non_zero_u64(&self) -> NonZeroU64 {
        self.0
    }
}

impl<'a> From<&'a Id> for Option<Id> {
    fn from(id: &'a Id) -> Self {
        Some(id.clone())
    }
}

impl<'a> Attributes<'a> {
    #[must_use]
    pub const fn new(
        metadata: &'static Metadata<'static>,
        values: &'a field::ValueSet<'a>,
    ) -> Self {
        Self {
            metadata,
            values,
            parent: Parent::Current,
        }
    }

    #[must_use]
    pub const fn new_root(
        metadata: &'static Metadata<'static>,
        values: &'a field::ValueSet<'a>,
    ) -> Self {
        Self {
            metadata,
            values,
            parent: Parent::Root,
        }
    }

    #[must_use]
    pub const fn child_of(
        parent: Id,
        metadata: &'static Metadata<'static>,
        values: &'a field::ValueSet<'a>,
    ) -> Self {
        Self {
            metadata,
            values,
            parent: Parent::Explicit(parent),
        }
    }

    #[must_use]
    pub const fn metadata(&self) -> &'static Metadata<'static> {
        self.metadata
    }

    #[must_use]
    pub const fn values(&self) -> &field::ValueSet<'a> {
        self.values
    }

    #[must_use]
    pub const fn is_root(&self) -> bool {
        matches!(self.parent, Parent::Root)
    }

    #[must_use]
    pub const fn is_contextual(&self) -> bool {
        matches!(self.parent, Parent::Current)
    }

    #[must_use]
    pub const fn parent(&self) -> Option<&Id> {
        match &self.parent {
            Parent::Explicit(parent) => Some(parent),
            _ => None,
        }
    }

    pub fn record(&self, visitor: &mut dyn field::Visit) {
        self.values.record(visitor);
    }

    #[must_use]
    pub fn contains(&self, field: &field::Field) -> bool {
        self.values.contains(field)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    #[must_use]
    pub const fn fields(&self) -> &FieldSet {
        self.values.field_set()
    }
}

impl<'a> Record<'a> {
    #[must_use]
    pub const fn new(values: &'a field::ValueSet<'a>) -> Self {
        Self { values }
    }

    pub fn record(&self, visitor: &mut dyn field::Visit) {
        self.values.record(visitor);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn contains(&self, field: &field::Field) -> bool {
        self.values.contains(field)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl Current {
    #[must_use]
    pub const fn new(id: Id, metadata: &'static Metadata<'static>) -> Self {
        Self {
            inner: CurrentInner::Current { id, metadata },
        }
    }

    #[must_use]
    pub const fn none() -> Self {
        Self {
            inner: CurrentInner::None,
        }
    }

    pub(crate) const fn unknown() -> Self {
        Self {
            inner: CurrentInner::Unknown,
        }
    }

    #[must_use]
    pub const fn is_known(&self) -> bool {
        !matches!(self.inner, CurrentInner::Unknown)
    }

    #[must_use]
    pub const fn into_inner(self) -> Option<(Id, &'static Metadata<'static>)> {
        match self.inner {
            CurrentInner::Current { id, metadata } => Some((id, metadata)),
            _ => None,
        }
    }

    #[must_use]
    pub const fn id(&self) -> Option<&Id> {
        match &self.inner {
            CurrentInner::Current { id, .. } => Some(id),
            _ => None,
        }
    }

    #[must_use]
    pub const fn metadata(&self) -> Option<&'static Metadata<'static>> {
        match self.inner {
            CurrentInner::Current { metadata, .. } => Some(metadata),
            _ => None,
        }
    }
}

impl<'a> From<&'a Current> for Option<&'a Id> {
    fn from(current: &'a Current) -> Self {
        current.id()
    }
}

impl<'a> From<&'a Current> for Option<Id> {
    fn from(current: &'a Current) -> Self {
        current.id().cloned()
    }
}

impl From<Current> for Option<Id> {
    fn from(current: Current) -> Self {
        current.into_inner().map(|(id, _)| id)
    }
}

pub struct Span {
    inner: Option<Inner>,
    metadata: Option<&'static Metadata<'static>>,
}

struct Inner {
    id: Id,
    dispatch: &'static Dispatch,
}

#[must_use = "once a span has been entered, it should be exited"]
pub struct Entered<'a> {
    span: &'a Span,
}

#[must_use = "once a span has been entered, it should be exited"]
pub struct EnteredSpan {
    span: Span,
    not_send: PhantomData<*mut ()>,
}

impl Span {
    #[must_use]
    pub fn new(metadata: &'static Metadata<'static>, values: &field::ValueSet<'_>) -> Self {
        dispatcher::get_default(|dispatch| {
            Self::make(metadata, &Attributes::new(metadata, values), dispatch)
        })
    }

    #[must_use]
    pub fn new_root(metadata: &'static Metadata<'static>, values: &field::ValueSet<'_>) -> Self {
        dispatcher::get_default(|dispatch| {
            Self::make(metadata, &Attributes::new_root(metadata, values), dispatch)
        })
    }

    #[must_use]
    pub fn child_of(
        parent: impl Into<Option<Id>>,
        metadata: &'static Metadata<'static>,
        values: &field::ValueSet<'_>,
    ) -> Self {
        let parent = parent.into();
        dispatcher::get_default(move |dispatch| {
            let attributes = match parent {
                Some(parent) => Attributes::child_of(parent, metadata, values),
                None => Attributes::new_root(metadata, values),
            };
            Self::make(metadata, &attributes, dispatch)
        })
    }

    #[must_use]
    pub const fn new_disabled(metadata: &'static Metadata<'static>) -> Self {
        Self {
            inner: None,
            metadata: Some(metadata),
        }
    }

    #[must_use]
    pub const fn none() -> Self {
        Self {
            inner: None,
            metadata: None,
        }
    }

    #[must_use]
    pub fn current() -> Self {
        dispatcher::get_default(|dispatch| match dispatch.current_span().into_inner() {
            Some((id, metadata)) => Self {
                inner: Some(Inner {
                    id: dispatch.clone_span(&id),
                    dispatch,
                }),
                metadata: Some(metadata),
            },
            None => Self::none(),
        })
    }

    fn make(
        metadata: &'static Metadata<'static>,
        attributes: &Attributes<'_>,
        dispatch: &'static Dispatch,
    ) -> Self {
        Self {
            inner: Some(Inner {
                id: dispatch.new_span(attributes),
                dispatch,
            }),
            metadata: Some(metadata),
        }
    }

    #[inline(always)]
    pub fn enter(&self) -> Entered<'_> {
        self.do_enter();
        Entered { span: self }
    }

    #[inline(always)]
    pub fn entered(self) -> EnteredSpan {
        self.do_enter();
        EnteredSpan {
            span: self,
            not_send: PhantomData,
        }
    }

    #[must_use]
    pub fn or_current(self) -> Self {
        if self.is_disabled() {
            return Self::current();
        }
        self
    }

    #[inline(always)]
    fn do_enter(&self) {
        if let Some(inner) = &self.inner {
            inner.dispatch.enter(&inner.id);
        }
    }

    #[inline(always)]
    fn do_exit(&self) {
        if let Some(inner) = &self.inner {
            inner.dispatch.exit(&inner.id);
        }
    }

    pub fn in_scope<F: FnOnce() -> T, T>(&self, run: F) -> T {
        let _enter = self.enter();
        run()
    }

    pub fn field<Q: AsField + ?Sized>(&self, field: &Q) -> Option<field::Field> {
        self.metadata.and_then(|metadata| field.as_field(metadata))
    }

    pub fn has_field<Q: AsField + ?Sized>(&self, field: &Q) -> bool {
        self.field(field).is_some()
    }

    pub fn record<Q: AsField + ?Sized, V: field::Value>(&self, field: &Q, value: V) -> &Self {
        if let Some(metadata) = self.metadata
            && let Some(field) = field.as_field(metadata)
        {
            self.record_all(
                &metadata
                    .fields()
                    .value_set(&[(&field, Some(&value as &dyn field::Value))]),
            );
        }
        self
    }

    pub fn record_all(&self, values: &field::ValueSet<'_>) -> &Self {
        if let Some(inner) = &self.inner {
            inner.dispatch.record(&inner.id, &Record::new(values));
        }
        self
    }

    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.inner.is_none()
    }

    #[must_use]
    pub const fn is_none(&self) -> bool {
        self.is_disabled() && self.metadata.is_none()
    }

    pub fn follows_from(&self, from: impl Into<Option<Id>>) -> &Self {
        if let Some(inner) = &self.inner
            && let Some(from) = from.into()
        {
            inner.dispatch.record_follows_from(&inner.id, &from);
        }
        self
    }

    #[must_use]
    pub fn id(&self) -> Option<Id> {
        self.inner.as_ref().map(|inner| inner.id.clone())
    }

    #[must_use]
    pub const fn metadata(&self) -> Option<&'static Metadata<'static>> {
        self.metadata
    }

    pub fn with_subscriber<T>(&self, read: impl FnOnce((&Id, &Dispatch)) -> T) -> Option<T> {
        self.inner
            .as_ref()
            .map(|inner| read((&inner.id, inner.dispatch)))
    }
}

impl Clone for Span {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.as_ref().map(|inner| Inner {
                id: inner.dispatch.clone_span(&inner.id),
                dispatch: inner.dispatch,
            }),
            metadata: self.metadata,
        }
    }
}

impl cmp::PartialEq for Span {
    fn eq(&self, other: &Self) -> bool {
        match (&self.metadata, &other.metadata) {
            (Some(this), Some(that)) => {
                this.callsite() == that.callsite()
                    && self.inner.as_ref().map(|inner| &inner.id)
                        == other.inner.as_ref().map(|inner| &inner.id)
            }
            _ => false,
        }
    }
}

impl Hash for Span {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.inner.as_ref().map(|inner| &inner.id).hash(hasher);
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut span = formatter.debug_struct("Span");
        if let Some(metadata) = self.metadata {
            span.field("name", &metadata.name())
                .field("level", &metadata.level())
                .field("target", &metadata.target());
            match &self.inner {
                Some(inner) => span.field("id", &inner.id),
                None => span.field("disabled", &true),
            };
            if let Some(path) = metadata.module_path() {
                span.field("module_path", &path);
            }
            if let Some(line) = metadata.line() {
                span.field("line", &line);
            }
            if let Some(file) = metadata.file() {
                span.field("file", &file);
            }
        } else {
            span.field("none", &true);
        }
        span.finish()
    }
}

impl<'a> From<&'a Span> for Option<&'a Id> {
    fn from(span: &'a Span) -> Self {
        span.inner.as_ref().map(|inner| &inner.id)
    }
}

impl<'a> From<&'a Span> for Option<Id> {
    fn from(span: &'a Span) -> Self {
        span.id()
    }
}

impl From<Span> for Option<Id> {
    fn from(span: Span) -> Self {
        span.id()
    }
}

impl<'a> From<&'a EnteredSpan> for Option<&'a Id> {
    fn from(span: &'a EnteredSpan) -> Self {
        span.span.inner.as_ref().map(|inner| &inner.id)
    }
}

impl<'a> From<&'a EnteredSpan> for Option<Id> {
    fn from(span: &'a EnteredSpan) -> Self {
        span.span.id()
    }
}

impl Drop for Span {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some(inner) = &self.inner {
            inner.dispatch.try_close(inner.id.clone());
        }
    }
}

impl EnteredSpan {
    #[must_use]
    pub fn id(&self) -> Option<Id> {
        self.span.id()
    }

    #[inline]
    pub fn exit(mut self) -> Span {
        let span = mem::replace(&mut self.span, Span::none());
        span.do_exit();
        span
    }
}

impl Deref for EnteredSpan {
    type Target = Span;

    fn deref(&self) -> &Span {
        &self.span
    }
}

impl fmt::Debug for Entered<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Entered")
            .field("span", self.span)
            .finish()
    }
}

impl fmt::Debug for EnteredSpan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnteredSpan")
            .field("span", &self.span)
            .field("not_send", &self.not_send)
            .finish()
    }
}

impl Drop for Entered<'_> {
    #[inline(always)]
    fn drop(&mut self) {
        self.span.do_exit();
    }
}

impl Drop for EnteredSpan {
    #[inline(always)]
    fn drop(&mut self) {
        self.span.do_exit();
    }
}
