use std::{
    any::Any,
    cell::{Cell, RefCell},
    marker::PhantomData,
    ops::Deref,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering, fence},
    },
};

use nanocode_sync::{AtomMap, SwapOption};
use tracing_core::{
    Event, Interest, LevelFilter, Metadata,
    span::{Attributes, Current, Id, Record},
};

pub const LAYER_SLOTS: usize = 3;

const PAGE_FIRST_ENTRIES: usize = 32;
const PAGE_COUNT: usize = 27;
const TABLE_ENTRIES: u64 = (PAGE_FIRST_ENTRIES as u64) * ((1 << PAGE_COUNT) - 1);
const INDEX_BITS: u32 = 32;
const INDEX_MASK: u64 = (1 << INDEX_BITS) - 1;
const FREE_SHARDS: usize = 16;
const NO_PARENT: u64 = 0;

pub trait Filter: Send + Sync + 'static {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool;

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        let _ = metadata;
        Interest::sometimes()
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        None
    }

    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id) {
        let _ = (attributes, id);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>) {
        let _ = (id, values);
    }

    fn on_enter(&self, id: &Id) {
        let _ = id;
    }

    fn on_exit(&self, id: &Id) {
        let _ = id;
    }

    fn on_close(&self, id: &Id) {
        let _ = id;
    }
}

impl Filter for Box<dyn Filter> {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        (**self).enabled(metadata)
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        (**self).callsite_enabled(metadata)
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        (**self).max_level_hint()
    }

    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id) {
        (**self).on_new_span(attributes, id);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>) {
        (**self).on_record(id, values);
    }

    fn on_enter(&self, id: &Id) {
        (**self).on_enter(id);
    }

    fn on_exit(&self, id: &Id) {
        (**self).on_exit(id);
    }

    fn on_close(&self, id: &Id) {
        (**self).on_close(id);
    }
}

pub trait Layer: Send + Sync + 'static {
    type Span: Send + Sync + 'static;

    fn register_callsite(&self, metadata: &'static Metadata<'static>) {
        let _ = metadata;
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let _ = metadata;
        true
    }

    fn new_span(
        &self,
        attributes: &Attributes<'_>,
        span: &SpanRef<'_, Self::Span>,
        context: &Context<'_, Self::Span>,
    ) -> Self::Span;

    fn on_new_span(
        &self,
        attributes: &Attributes<'_>,
        span: &SpanRef<'_, Self::Span>,
        context: &Context<'_, Self::Span>,
    ) {
        let _ = (attributes, span, context);
    }

    fn on_record(
        &self,
        span: &SpanRef<'_, Self::Span>,
        values: &Record<'_>,
        context: &Context<'_, Self::Span>,
    ) {
        let _ = (span, values, context);
    }

    fn on_follows_from(
        &self,
        span: &SpanRef<'_, Self::Span>,
        follows: &SpanRef<'_, Self::Span>,
        context: &Context<'_, Self::Span>,
    ) {
        let _ = (span, follows, context);
    }

    fn on_event(&self, event: &Event<'_>, context: &Context<'_, Self::Span>) {
        let _ = (event, context);
    }

    fn on_enter(&self, span: &SpanRef<'_, Self::Span>, context: &Context<'_, Self::Span>) {
        let _ = (span, context);
    }

    fn on_exit(&self, span: &SpanRef<'_, Self::Span>, context: &Context<'_, Self::Span>) {
        let _ = (span, context);
    }

    fn on_close(&self, span: &SpanRef<'_, Self::Span>, context: &Context<'_, Self::Span>) {
        let _ = (span, context);
    }
}

pub struct Filtered(Box<dyn Slot>);

impl Filtered {
    pub fn new<L: Layer, F: Filter>(layer: L, filter: F) -> Self {
        Self(Box::new(Pair {
            layer,
            filter,
            inner: None,
        }))
    }

    pub fn nested<L: Layer, I: Filter, F: Filter>(layer: L, inner: I, filter: F) -> Self {
        Self(Box::new(Pair {
            layer,
            filter,
            inner: Some(Box::new(inner)),
        }))
    }
}

struct Pair<L, F> {
    layer: L,
    filter: F,
    inner: Option<Box<dyn Filter>>,
}

const fn outer_bit(slot: usize) -> usize {
    2 * slot
}

const fn inner_bit(slot: usize) -> usize {
    2 * slot + 1
}

trait Slot: Send + Sync {
    fn callsite_interests(
        &self,
        metadata: &'static Metadata<'static>,
        add: &mut dyn FnMut(Interest),
    );

    fn enabled(&self, slot: usize, metadata: &Metadata<'_>) -> bool;

    fn max_level_hint(&self) -> Option<LevelFilter>;

    fn on_new_span(&self, registry: &Registry, slot: usize, attributes: &Attributes<'_>, id: &Id);

    fn on_record(&self, span: Visible<'_>, values: &Record<'_>);

    fn on_follows_from(&self, span: Visible<'_>, follows: Visible<'_>);

    fn on_event(&self, registry: &Registry, slot: usize, event: &Event<'_>);

    fn on_enter(&self, span: Visible<'_>);

    fn on_exit(&self, span: Visible<'_>);

    fn on_close(&self, span: Visible<'_>);
}

#[derive(Clone, Copy)]
struct Visible<'a> {
    registry: &'a Registry,
    slot: usize,
    entry: &'a Entry,
    id: u64,
}

impl<'a> Visible<'a> {
    const fn split<D>(self) -> (SpanRef<'a, D>, Context<'a, D>) {
        (
            SpanRef::new(self.registry, self.entry, self.id, self.slot),
            Context::new(self.registry, self.slot),
        )
    }

    fn outer_enabled(&self) -> bool {
        self.entry.bit_enabled(outer_bit(self.slot))
    }

    fn inner_enabled(&self) -> bool {
        self.entry.bit_enabled(inner_bit(self.slot))
    }

    fn id(&self) -> Id {
        Id::from_u64(self.id)
    }
}

impl<L: Layer, F: Filter> Pair<L, F> {
    fn inner_visible(&self, span: &Visible<'_>, run: impl FnOnce(Option<&dyn Filter>)) {
        match &self.inner {
            None => run(None),
            Some(inner) if span.inner_enabled() => run(Some(&**inner)),
            Some(_) => {}
        }
    }
}

impl<L: Layer, F: Filter> Slot for Pair<L, F> {
    fn callsite_interests(
        &self,
        metadata: &'static Metadata<'static>,
        add: &mut dyn FnMut(Interest),
    ) {
        let outer = self.filter.callsite_enabled(metadata);
        if !outer.is_never() {
            match &self.inner {
                None => self.layer.register_callsite(metadata),
                Some(inner) => {
                    let interest = inner.callsite_enabled(metadata);
                    if !interest.is_never() {
                        self.layer.register_callsite(metadata);
                    }
                    add(interest);
                }
            }
        }
        add(outer);
    }

    fn enabled(&self, slot: usize, metadata: &Metadata<'_>) -> bool {
        let enabled = self.filter.enabled(metadata);
        FilterMap::current().set(outer_bit(slot), enabled).store();
        if !enabled {
            return true;
        }
        if let Some(inner) = &self.inner {
            let enabled = inner.enabled(metadata);
            FilterMap::current().set(inner_bit(slot), enabled).store();
            if !enabled {
                return true;
            }
        }
        self.layer.enabled(metadata)
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        self.filter.max_level_hint()
    }

    fn on_new_span(&self, registry: &Registry, slot: usize, attributes: &Attributes<'_>, id: &Id) {
        did_enable(outer_bit(slot), || {
            self.filter.on_new_span(attributes, id);
            match &self.inner {
                None => self.new_span(registry, slot, attributes, id),
                Some(inner) => did_enable(inner_bit(slot), || {
                    inner.on_new_span(attributes, id);
                    self.new_span(registry, slot, attributes, id);
                }),
            }
        });
    }

    fn on_record(&self, span: Visible<'_>, values: &Record<'_>) {
        if !span.outer_enabled() {
            return;
        }
        self.filter.on_record(&span.id(), values);
        self.inner_visible(&span, |inner| {
            if let Some(inner) = inner {
                inner.on_record(&span.id(), values);
            }
            let (span, context) = span.split::<L::Span>();
            self.layer.on_record(&span, values, &context);
        });
    }

    fn on_follows_from(&self, span: Visible<'_>, follows: Visible<'_>) {
        if !(span.outer_enabled() && follows.outer_enabled()) {
            return;
        }
        if self.inner.is_some() && !(span.inner_enabled() && follows.inner_enabled()) {
            return;
        }
        let (follows, _) = follows.split::<L::Span>();
        let (span, context) = span.split::<L::Span>();
        self.layer.on_follows_from(&span, &follows, &context);
    }

    fn on_event(&self, registry: &Registry, slot: usize, event: &Event<'_>) {
        let deliver = || {
            self.layer
                .on_event(event, &Context::<L::Span>::new(registry, slot));
        };
        did_enable(outer_bit(slot), || match &self.inner {
            None => deliver(),
            Some(_) => did_enable(inner_bit(slot), deliver),
        });
    }

    fn on_enter(&self, span: Visible<'_>) {
        if !span.outer_enabled() {
            return;
        }
        self.filter.on_enter(&span.id());
        self.inner_visible(&span, |inner| {
            if let Some(inner) = inner {
                inner.on_enter(&span.id());
            }
            let (span, context) = span.split::<L::Span>();
            self.layer.on_enter(&span, &context);
        });
    }

    fn on_exit(&self, span: Visible<'_>) {
        if !span.outer_enabled() {
            return;
        }
        self.filter.on_exit(&span.id());
        self.inner_visible(&span, |inner| {
            if let Some(inner) = inner {
                inner.on_exit(&span.id());
            }
            let (span, context) = span.split::<L::Span>();
            self.layer.on_exit(&span, &context);
        });
    }

    fn on_close(&self, span: Visible<'_>) {
        if !span.outer_enabled() {
            return;
        }
        self.filter.on_close(&span.id());
        self.inner_visible(&span, |inner| {
            if let Some(inner) = inner {
                inner.on_close(&span.id());
            }
            let (span, context) = span.split::<L::Span>();
            self.layer.on_close(&span, &context);
        });
    }
}

impl<L: Layer, F: Filter> Pair<L, F> {
    fn new_span(&self, registry: &Registry, slot: usize, attributes: &Attributes<'_>, id: &Id) {
        let context = Context::<L::Span>::new(registry, slot);
        let span = context
            .span(id)
            .expect("a span a layer's filter enabled is visible to that layer");
        let data = self.layer.new_span(attributes, &span, &context);
        let stored: Box<dyn Any + Send + Sync> = Box::new(data);
        let previous = span.entry.data[slot].swap(Some(Arc::new(stored)));
        assert!(previous.is_none(), "a layer's span data is set once, at new_span");
        self.layer.on_new_span(attributes, &span, &context);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FilterMap(u64);

impl FilterMap {
    const ENABLED: Self = Self(0);
    const DISABLED_BY_ALL: u64 = u64::MAX;
    const SLOT_BITS: u64 = 0b11;

    fn current() -> Self {
        FILTERING.with(Cell::get)
    }

    fn store(self) {
        FILTERING.with(|filtering| filtering.set(self));
    }

    const fn set(self, bit: usize, enabled: bool) -> Self {
        let mask = 1 << bit;
        if enabled {
            Self(self.0 & !mask)
        } else {
            Self(self.0 | mask)
        }
    }

    const fn bit_enabled(self, bit: usize) -> bool {
        self.0 & (1 << bit) == 0
    }

    const fn is_enabled(self, slot: usize) -> bool {
        self.0 & (Self::SLOT_BITS << outer_bit(slot)) == 0
    }

    const fn any_enabled(self) -> bool {
        self.0 != Self::DISABLED_BY_ALL
    }
}


thread_local! {
    static FILTERING: Cell<FilterMap> = const { Cell::new(FilterMap::ENABLED) };
    static STACKS: RefCell<Vec<Stack>> = const { RefCell::new(Vec::new()) };
    static FREE_SHARD: usize = NEXT_FREE_SHARD.fetch_add(1, Ordering::Relaxed) % FREE_SHARDS;
}

static NEXT_FREE_SHARD: AtomicUsize = AtomicUsize::new(0);
static NEXT_REGISTRY: AtomicU64 = AtomicU64::new(0);

struct Stack {
    registry: u64,
    entered: Vec<Entered>,
}

struct Entered {
    id: Id,
    duplicate: bool,
}

struct Paged<T> {
    pages: [OnceLock<Box<[T]>>; PAGE_COUNT],
}

impl<T: Default> Paged<T> {
    fn new() -> Self {
        Self {
            pages: std::array::from_fn(|_| OnceLock::new()),
        }
    }

    fn locate(index: u64) -> (usize, usize) {
        assert!(index < TABLE_ENTRIES, "a table index is below the table's size");
        let first = PAGE_FIRST_ENTRIES as u64;
        let ordinal = index / first + 1;
        let page = (u64::BITS - 1 - ordinal.leading_zeros()) as usize;
        let start = first * ((1 << page) - 1);
        let offset = usize::try_from(index - start).expect("a page offset fits in usize");
        (page, offset)
    }

    fn entry(&self, index: u64) -> &T {
        let (page, offset) = Self::locate(index);
        let entries = self.pages[page].get_or_init(|| {
            (0..PAGE_FIRST_ENTRIES << page).map(|_| T::default()).collect()
        });
        &entries[offset]
    }

    fn existing(&self, index: u64) -> Option<&T> {
        let (page, offset) = Self::locate(index);
        self.pages[page].get().map(|entries| &entries[offset])
    }
}

type SpanData = Arc<Box<dyn Any + Send + Sync>>;

#[repr(align(128))]
struct FreeShard(AtomicU64);

#[repr(align(128))]
struct Entry {
    generation: AtomicU32,
    references: AtomicUsize,
    metadata: AtomicU32,
    parent: AtomicU64,
    filter_map: AtomicU64,
    data: [SwapOption<Box<dyn Any + Send + Sync>>; LAYER_SLOTS],
    next_free: AtomicU32,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            generation: AtomicU32::new(0),
            references: AtomicUsize::new(0),
            metadata: AtomicU32::new(0),
            parent: AtomicU64::new(NO_PARENT),
            filter_map: AtomicU64::new(0),
            data: std::array::from_fn(|_| SwapOption::empty()),
            next_free: AtomicU32::new(0),
        }
    }
}

impl Entry {
    fn is_enabled_for(&self, slot: usize) -> bool {
        FilterMap(self.filter_map.load(Ordering::Relaxed)).is_enabled(slot)
    }

    fn bit_enabled(&self, bit: usize) -> bool {
        FilterMap(self.filter_map.load(Ordering::Relaxed)).bit_enabled(bit)
    }

    fn parent(&self) -> Option<Id> {
        match self.parent.load(Ordering::Relaxed) {
            NO_PARENT => None,
            parent => Some(Id::from_u64(parent)),
        }
    }
}

struct Callsites {
    metadata: Paged<OnceLock<&'static Metadata<'static>>>,
    next: AtomicU64,
    by_address: AtomMap<usize, u32>,
}

impl Callsites {
    fn intern(&self, metadata: &'static Metadata<'static>) -> u32 {
        let address = std::ptr::from_ref(metadata).addr();
        if let Some(index) = self.by_address.get(&address) {
            return index;
        }
        let index = self.next.fetch_add(1, Ordering::Relaxed);
        assert!(
            self.metadata.entry(index).set(metadata).is_ok(),
            "an interned callsite slot is written once"
        );
        let index = u32::try_from(index).expect("callsite indexes fit in u32");
        self.by_address.insert(address, index);
        index
    }

    fn get(&self, index: u32) -> &'static Metadata<'static> {
        self.metadata
            .existing(u64::from(index))
            .and_then(OnceLock::get)
            .expect("an interned callsite index names stored metadata")
    }
}

struct Table {
    entries: Paged<Entry>,
    unused: AtomicU64,
    free: [FreeShard; FREE_SHARDS],
}

struct NewSpan {
    metadata: u32,
    parent: Option<Id>,
    filter_map: FilterMap,
}

impl Table {
    fn new() -> Self {
        Self {
            entries: Paged::new(),
            unused: AtomicU64::new(0),
            free: std::array::from_fn(|_| FreeShard(AtomicU64::new(0))),
        }
    }

    fn insert(&self, span: &NewSpan) -> Id {
        let index = self.pop_free().unwrap_or_else(|| {
            let index = self.unused.fetch_add(1, Ordering::Relaxed);
            assert!(index < TABLE_ENTRIES, "Unable to allocate another span");
            index
        });
        let entry = self.entries.entry(index);
        let generation = u64::from(entry.generation.load(Ordering::Acquire));
        entry.metadata.store(span.metadata, Ordering::Relaxed);
        entry.parent.store(
            span.parent.as_ref().map_or(NO_PARENT, Id::into_u64),
            Ordering::Relaxed,
        );
        entry.filter_map.store(span.filter_map.0, Ordering::Relaxed);
        let previous = entry.references.swap(1, Ordering::Release);
        assert_eq!(previous, 0, "a free table entry holds no references");
        Id::from_u64((generation << INDEX_BITS) | (index + 1))
    }

    fn get(&self, id: &Id) -> Option<&Entry> {
        let raw = id.into_u64();
        let index = (raw & INDEX_MASK).checked_sub(1)?;
        if index >= TABLE_ENTRIES {
            return None;
        }
        let entry = self.entries.existing(index)?;
        (u64::from(entry.generation.load(Ordering::Acquire)) == raw >> INDEX_BITS).then_some(entry)
    }

    fn remove(&self, id: &Id, entry: &Entry) {
        for data in &entry.data {
            if data.load().is_some() {
                drop(data.swap(None));
            }
        }
        let generation = entry.generation.fetch_add(1, Ordering::AcqRel);
        assert_eq!(
            u64::from(generation),
            id.into_u64() >> INDEX_BITS,
            "a closed span is removed once"
        );
        self.push_free((id.into_u64() & INDEX_MASK) - 1);
    }

    fn pop_free(&self) -> Option<u64> {
        let own = FREE_SHARD.with(|shard| *shard);
        (0..FREE_SHARDS).find_map(|offset| self.pop_shard((own + offset) % FREE_SHARDS))
    }

    fn pop_shard(&self, shard: usize) -> Option<u64> {
        let free = &self.free[shard].0;
        let mut head = free.load(Ordering::Acquire);
        loop {
            let index = (head & INDEX_MASK).checked_sub(1)?;
            let next = u64::from(self.entries.entry(index).next_free.load(Ordering::Acquire));
            let tag = (head >> INDEX_BITS).wrapping_add(1) & INDEX_MASK;
            match free.compare_exchange_weak(
                head,
                (tag << INDEX_BITS) | next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(index),
                Err(actual) => head = actual,
            }
        }
    }

    fn push_free(&self, index: u64) {
        let entry = self.entries.entry(index);
        let free = &self.free[FREE_SHARD.with(|shard| *shard)].0;
        let mut head = free.load(Ordering::Acquire);
        loop {
            let top = u32::try_from(head & INDEX_MASK).expect("a free list index fits in u32");
            entry.next_free.store(top, Ordering::Release);
            let tag = (head >> INDEX_BITS).wrapping_add(1) & INDEX_MASK;
            match free.compare_exchange_weak(
                head,
                (tag << INDEX_BITS) | (index + 1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(actual) => head = actual,
            }
        }
    }
}

pub struct Registry {
    spans: Table,
    callsites: Callsites,
    slots: [Option<Filtered>; LAYER_SLOTS],
    key: u64,
}

enum Release {
    Kept,
    Closed(Option<Id>),
}

impl Registry {
    #[must_use]
    pub fn new(slots: [Option<Filtered>; LAYER_SLOTS]) -> Self {
        Self {
            spans: Table::new(),
            callsites: Callsites {
                metadata: Paged::new(),
                next: AtomicU64::new(0),
                by_address: AtomMap::new(),
            },
            slots,
            key: NEXT_REGISTRY.fetch_add(1, Ordering::Relaxed),
        }
    }

    fn has_filters(&self) -> bool {
        self.slots.iter().any(Option::is_some)
    }

    fn present(&self) -> impl Iterator<Item = (usize, &dyn Slot)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, layer)| layer.as_ref().map(|layer| (slot, &*layer.0)))
    }

    fn present_outermost_first(&self) -> impl Iterator<Item = (usize, &dyn Slot)> {
        self.slots
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(slot, layer)| layer.as_ref().map(|layer| (slot, &*layer.0)))
    }

    fn visible<'a>(&'a self, entry: &'a Entry, id: &Id, slot: usize) -> Visible<'a> {
        Visible {
            registry: self,
            slot,
            entry,
            id: id.into_u64(),
        }
    }

    pub fn data<D: Send + Sync + 'static, R>(
        &self,
        id: &Id,
        read: impl FnOnce(&D) -> R,
    ) -> Option<R> {
        let entry = self.spans.get(id)?;
        let data = entry.data.iter().find_map(|data| {
            let data = data.load_full()?;
            data.downcast_ref::<D>().is_some().then_some(data)
        })?;
        Some(read(data.downcast_ref::<D>().expect("the data was found by its type")))
    }

    fn with_stack<R>(&self, read: impl FnOnce(Option<&mut Vec<Entered>>) -> R) -> R {
        STACKS.with(|stacks| {
            let mut stacks = stacks.borrow_mut();
            let stack = stacks
                .iter_mut()
                .find(|stack| stack.registry == self.key)
                .map(|stack| &mut stack.entered);
            read(stack)
        })
    }

    fn push_entered(&self, id: &Id) {
        STACKS.with(|stacks| {
            let mut stacks = stacks.borrow_mut();
            let position = match stacks.iter().position(|stack| stack.registry == self.key) {
                Some(position) => position,
                None => {
                    stacks.push(Stack {
                        registry: self.key,
                        entered: Vec::new(),
                    });
                    stacks.len() - 1
                }
            };
            let entered = &mut stacks[position].entered;
            let duplicate = entered.iter().any(|entry| entry.id == *id);
            entered.push(Entered {
                id: id.clone(),
                duplicate,
            });
        });
    }

    fn pop_entered(&self, id: &Id) {
        STACKS.with(|stacks| {
            let mut stacks = stacks.borrow_mut();
            let Some(position) = stacks.iter().position(|stack| stack.registry == self.key)
            else {
                return;
            };
            let entered = &mut stacks[position].entered;
            if let Some(index) = entered.iter().rposition(|entry| entry.id == *id) {
                entered.remove(index);
            }
            if entered.is_empty() {
                stacks.swap_remove(position);
            }
        });
    }

    fn current_id(&self) -> Option<Id> {
        self.with_stack(|stack| {
            stack?
                .iter()
                .rev()
                .find(|entry| !entry.duplicate)
                .map(|entry| entry.id.clone())
        })
    }

    fn current_visible(&self, slot: usize) -> Option<(Id, &Entry)> {
        let current = self.current_id()?;
        let entry = self.spans.get(&current)?;
        if entry.is_enabled_for(slot) {
            return Some((current, entry));
        }
        let entered = self.with_stack(|stack| {
            stack.map_or_else(Vec::new, |stack| {
                stack
                    .iter()
                    .rev()
                    .filter(|entry| !entry.duplicate)
                    .map(|entry| entry.id.clone())
                    .collect::<Vec<_>>()
            })
        });
        entered.into_iter().find_map(|id| {
            let entry = self.spans.get(&id)?;
            entry.is_enabled_for(slot).then_some((id, entry))
        })
    }

    fn interest(&self, metadata: &'static Metadata<'static>) -> Interest {
        let mut combined: Option<Interest> = None;
        for (_, slot) in self.present_outermost_first() {
            slot.callsite_interests(metadata, &mut |interest| {
                combined = Some(match combined.take() {
                    None => interest,
                    Some(current)
                        if (current.is_always() && !interest.is_always())
                            || (current.is_never() && !interest.is_never()) =>
                    {
                        Interest::sometimes()
                    }
                    Some(current) => current,
                });
            });
        }
        let mut interest = if self.has_filters() {
            combined.unwrap_or_else(Interest::always)
        } else {
            Interest::always()
        };
        for depth in 1..=LAYER_SLOTS {
            let inner_has_filters =
                depth == 1 || self.slots[..depth - 1].iter().any(Option::is_some);
            if self.slots[depth - 1].is_none() && interest.is_never() && inner_has_filters {
                interest = Interest::sometimes();
            }
        }
        interest
    }

    fn level_hint(&self) -> Option<LevelFilter> {
        let mut hint = None;
        for depth in 1..=LAYER_SLOTS {
            let layer = self.slots[depth - 1].as_ref();
            let outer = layer.map_or(Some(LevelFilter::OFF), |layer| layer.0.max_level_hint());
            let inner = &self.slots[..depth - 1];
            hint = pick_level_hint(&Chain {
                outer,
                inner: hint,
                has_layer_filter: layer.is_some(),
                inner_is_registry: depth == 1,
                inner_has_layer_filter: depth == 1 || inner.iter().any(Option::is_some),
                inner_is_none: inner.iter().any(Option::is_none),
            });
        }
        hint
    }

    fn release(&self, id: &Id) -> Release {
        let entry = match self.spans.get(id) {
            Some(entry) => entry,
            None if std::thread::panicking() => return Release::Kept,
            None => panic!("tried to drop a ref to {id:?}, but no such span exists!"),
        };
        let references = entry.references.fetch_sub(1, Ordering::Release);
        if !std::thread::panicking() {
            assert!(references < usize::MAX, "reference count overflow!");
        }
        if references > 1 {
            return Release::Kept;
        }
        fence(Ordering::Acquire);
        for (slot, layer) in self.present() {
            layer.on_close(self.visible(entry, id, slot));
        }
        let parent = entry.parent();
        self.spans.remove(id, entry);
        Release::Closed(parent)
    }
}

struct Chain {
    outer: Option<LevelFilter>,
    inner: Option<LevelFilter>,
    has_layer_filter: bool,
    inner_is_registry: bool,
    inner_has_layer_filter: bool,
    inner_is_none: bool,
}

fn pick_level_hint(chain: &Chain) -> Option<LevelFilter> {
    if chain.inner_is_registry {
        return chain.outer;
    }
    if chain.has_layer_filter && chain.inner_has_layer_filter {
        return Some(std::cmp::max(chain.outer?, chain.inner?));
    }
    if chain.has_layer_filter && chain.inner.is_none() {
        return None;
    }
    if chain.inner_has_layer_filter && chain.outer.is_none() {
        return None;
    }
    if !chain.has_layer_filter {
        return std::cmp::max(chain.outer, Some(chain.inner?));
    }
    if chain.inner_is_none && chain.inner == Some(LevelFilter::OFF) {
        return chain.outer;
    }
    std::cmp::max(chain.outer, chain.inner)
}

fn did_enable(bit: usize, run: impl FnOnce()) {
    let map = FilterMap::current();
    if map.bit_enabled(bit) {
        run();
    } else {
        map.set(bit, true).store();
    }
}

impl tracing_core::Subscriber for Registry {
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        self.interest(metadata)
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        for (slot, layer) in self.present_outermost_first() {
            if !layer.enabled(slot, metadata) {
                FilterMap::ENABLED.store();
                return false;
            }
        }
        if self.has_filters() {
            return FilterMap::current().any_enabled();
        }
        true
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        self.level_hint()
    }

    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        let parent = if attributes.is_root() {
            None
        } else if attributes.is_contextual() {
            self.current_span().id().map(|id| self.clone_span(id))
        } else {
            attributes.parent().map(|id| self.clone_span(id))
        };
        let id = self.spans.insert(&NewSpan {
            metadata: self.callsites.intern(attributes.metadata()),
            parent,
            filter_map: FilterMap::current(),
        });
        for (slot, layer) in self.present() {
            layer.on_new_span(self, slot, attributes, &id);
        }
        id
    }

    fn record(&self, span: &Id, values: &Record<'_>) {
        let Some(entry) = self.spans.get(span) else {
            return;
        };
        for (slot, layer) in self.present() {
            layer.on_record(self.visible(entry, span, slot), values);
        }
    }

    fn record_follows_from(&self, span: &Id, follows: &Id) {
        let (Some(entry), Some(follows_entry)) = (self.spans.get(span), self.spans.get(follows))
        else {
            return;
        };
        for (slot, layer) in self.present() {
            layer.on_follows_from(
                self.visible(entry, span, slot),
                self.visible(follows_entry, follows, slot),
            );
        }
    }

    fn event(&self, event: &Event<'_>) {
        for (slot, layer) in self.present() {
            layer.on_event(self, slot, event);
        }
    }

    fn enter(&self, span: &Id) {
        self.push_entered(span);
        let Some(entry) = self.spans.get(span) else {
            return;
        };
        for (slot, layer) in self.present() {
            layer.on_enter(self.visible(entry, span, slot));
        }
    }

    fn exit(&self, span: &Id) {
        self.pop_entered(span);
        let Some(entry) = self.spans.get(span) else {
            return;
        };
        for (slot, layer) in self.present() {
            layer.on_exit(self.visible(entry, span, slot));
        }
    }

    fn clone_span(&self, id: &Id) -> Id {
        let entry = self.spans.get(id).unwrap_or_else(|| {
            panic!(
                "tried to clone {id:?}, but no span exists with that ID\nThis may be caused by consuming a parent span (`parent: span`) rather than borrowing it (`parent: &span`)."
            )
        });
        let references = entry.references.fetch_add(1, Ordering::Relaxed);
        assert_ne!(
            references, 0,
            "tried to clone a span ({id:?}) that already closed"
        );
        id.clone()
    }

    fn current_span(&self) -> Current {
        let Some(id) = self.current_id() else {
            return Current::none();
        };
        match self.spans.get(&id) {
            Some(entry) => {
                let metadata = self.callsites.get(entry.metadata.load(Ordering::Relaxed));
                Current::new(id, metadata)
            }
            None => Current::none(),
        }
    }

    fn try_close(&self, id: Id) -> bool {
        let Release::Closed(mut parent) = self.release(&id) else {
            return false;
        };
        while let Some(next) = parent {
            parent = match self.release(&next) {
                Release::Kept => None,
                Release::Closed(grandparent) => grandparent,
            };
        }
        true
    }
}

pub struct Context<'a, D> {
    registry: &'a Registry,
    slot: usize,
    span: PhantomData<fn() -> D>,
}

impl<'a, D> Context<'a, D> {
    const fn new(registry: &'a Registry, slot: usize) -> Self {
        Self {
            registry,
            slot,
            span: PhantomData,
        }
    }

    #[must_use]
    pub fn current_span(&self) -> Current {
        tracing_core::Subscriber::current_span(self.registry)
    }

    #[must_use]
    pub fn span(&self, id: &Id) -> Option<SpanRef<'a, D>> {
        let entry = self.registry.spans.get(id)?;
        entry
            .is_enabled_for(self.slot)
            .then(|| SpanRef::new(self.registry, entry, id.into_u64(), self.slot))
    }

    #[must_use]
    pub fn lookup_current(&self) -> Option<SpanRef<'a, D>> {
        let (id, entry) = self.registry.current_visible(self.slot)?;
        Some(SpanRef::new(self.registry, entry, id.into_u64(), self.slot))
    }

    #[must_use]
    pub fn event_span(&self, event: &Event<'_>) -> Option<SpanRef<'a, D>> {
        if event.is_root() {
            None
        } else if event.is_contextual() {
            self.lookup_current()
        } else {
            event.parent().and_then(|id| self.span(id))
        }
    }

    #[must_use]
    pub fn event_scope(&self, event: &Event<'_>) -> Option<Scope<'a, D>> {
        Some(self.event_span(event)?.scope())
    }
}

pub struct SpanRef<'a, D> {
    registry: &'a Registry,
    entry: &'a Entry,
    id: u64,
    slot: usize,
    span: PhantomData<fn() -> D>,
}

impl<'a, D> SpanRef<'a, D> {
    const fn new(registry: &'a Registry, entry: &'a Entry, id: u64, slot: usize) -> Self {
        Self {
            registry,
            entry,
            id,
            slot,
            span: PhantomData,
        }
    }

    #[must_use]
    pub fn id(&self) -> Id {
        Id::from_u64(self.id)
    }

    #[must_use]
    pub fn metadata(&self) -> &'static Metadata<'static> {
        self.registry
            .callsites
            .get(self.entry.metadata.load(Ordering::Relaxed))
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        self.metadata().name()
    }

    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let mut id = self.entry.parent()?;
        loop {
            let entry = self.registry.spans.get(&id)?;
            if entry.is_enabled_for(self.slot) {
                return Some(Self::new(self.registry, entry, id.into_u64(), self.slot));
            }
            id = entry.parent()?;
        }
    }

    #[must_use]
    pub fn scope(&self) -> Scope<'a, D> {
        Scope {
            registry: self.registry,
            next: Some(self.id()),
            slot: self.slot,
            span: PhantomData,
        }
    }
}

impl<D: 'static> SpanRef<'_, D> {
    #[must_use]
    pub fn data(&self) -> Option<Data<D>> {
        let data = self.entry.data[self.slot].load_full()?;
        assert!(
            data.downcast_ref::<D>().is_some(),
            "a layer's slot holds that layer's span data"
        );
        Some(Data {
            data,
            span: PhantomData,
        })
    }
}

pub struct Data<D> {
    data: SpanData,
    span: PhantomData<fn() -> D>,
}

impl<D: 'static> Deref for Data<D> {
    type Target = D;

    fn deref(&self) -> &D {
        self.data
            .downcast_ref::<D>()
            .expect("a layer's slot holds that layer's span data")
    }
}

pub struct Scope<'a, D> {
    registry: &'a Registry,
    next: Option<Id>,
    slot: usize,
    span: PhantomData<fn() -> D>,
}

impl<'a, D> Scope<'a, D> {
    pub fn from_root(self) -> std::iter::Rev<std::vec::IntoIter<SpanRef<'a, D>>> {
        self.collect::<Vec<_>>().into_iter().rev()
    }
}

impl<'a, D> Iterator for Scope<'a, D> {
    type Item = SpanRef<'a, D>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let id = self.next.take()?;
            let entry = self.registry.spans.get(&id)?;
            self.next = entry.parent();
            if entry.is_enabled_for(self.slot) {
                return Some(SpanRef::new(self.registry, entry, id.into_u64(), self.slot));
            }
        }
    }
}

