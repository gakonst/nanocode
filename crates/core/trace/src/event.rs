use crate::{
    Metadata, dispatcher, field,
    span::{Id, Parent},
};

#[derive(Debug)]
pub struct Event<'a> {
    fields: &'a field::ValueSet<'a>,
    metadata: &'static Metadata<'static>,
    parent: Parent,
}

impl<'a> Event<'a> {
    pub fn dispatch(metadata: &'static Metadata<'static>, fields: &'a field::ValueSet<'_>) {
        let event = Event::new(metadata, fields);
        dispatcher::get_default(|current| current.event(&event));
    }

    #[must_use]
    pub const fn new(
        metadata: &'static Metadata<'static>,
        fields: &'a field::ValueSet<'a>,
    ) -> Self {
        Event {
            fields,
            metadata,
            parent: Parent::Current,
        }
    }

    #[must_use]
    pub fn new_child_of(
        parent: impl Into<Option<Id>>,
        metadata: &'static Metadata<'static>,
        fields: &'a field::ValueSet<'a>,
    ) -> Self {
        let parent = match parent.into() {
            Some(parent) => Parent::Explicit(parent),
            None => Parent::Root,
        };
        Event {
            fields,
            metadata,
            parent,
        }
    }

    pub fn child_of(
        parent: impl Into<Option<Id>>,
        metadata: &'static Metadata<'static>,
        fields: &'a field::ValueSet<'_>,
    ) {
        let event = Self::new_child_of(parent, metadata, fields);
        dispatcher::get_default(|current| current.event(&event));
    }

    pub fn record(&self, visitor: &mut dyn field::Visit) {
        self.fields.record(visitor);
    }

    #[must_use]
    pub const fn fields(&self) -> field::Iter {
        self.fields.field_set().iter()
    }

    #[must_use]
    pub const fn metadata(&self) -> &'static Metadata<'static> {
        self.metadata
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
}
