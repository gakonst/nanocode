use std::{
    fmt,
    hash::{Hash, Hasher},
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{Metadata, dispatcher, metadata::LevelFilter, subscriber::Interest};

const INTEREST_BITS: u32 = 2;
const INTEREST_MASK: u32 = (1 << INTEREST_BITS) - 1;
const INTEREST_NEVER: u32 = 0;
const INTEREST_SOMETIMES: u32 = 1;
const INTEREST_ALWAYS: u32 = 2;
const GENERATION_UNREGISTERED: u32 = 0;
const GENERATION_FIRST: u32 = 1;
const GENERATION_MAX: u32 = u32::MAX >> INTEREST_BITS;

static GENERATION: AtomicU32 = AtomicU32::new(GENERATION_FIRST);

pub trait Callsite: Sync {
    fn metadata(&self) -> &Metadata<'_>;
}

pub struct DefaultCallsite {
    interest: AtomicU32,
    metadata: &'static Metadata<'static>,
}

#[derive(Clone, Copy)]
pub struct Identifier(&'static dyn Callsite);

pub fn rebuild_interest_cache() {
    let previous = GENERATION.fetch_add(1, Ordering::AcqRel);
    assert!(
        previous < GENERATION_MAX,
        "the interest cache is rebuilt fewer than {GENERATION_MAX} times"
    );
    let max_level =
        dispatcher::get_default(|current| current.max_level_hint().unwrap_or(LevelFilter::TRACE));
    LevelFilter::set_max(max_level);
}

impl DefaultCallsite {
    #[must_use]
    pub const fn new(metadata: &'static Metadata<'static>) -> Self {
        Self {
            interest: AtomicU32::new(GENERATION_UNREGISTERED << INTEREST_BITS),
            metadata,
        }
    }

    #[inline]
    pub fn interest(&'static self) -> Interest {
        let cached = self.interest.load(Ordering::Relaxed);
        if cached >> INTEREST_BITS == GENERATION.load(Ordering::Relaxed) {
            return decode(cached & INTEREST_MASK);
        }
        self.register()
    }

    #[cold]
    #[inline(never)]
    fn register(&'static self) -> Interest {
        let generation = GENERATION.load(Ordering::Acquire);
        let interest = dispatcher::get_default(|current| current.register_callsite(self.metadata));
        self.interest.store(
            (generation << INTEREST_BITS) | encode(&interest),
            Ordering::Relaxed,
        );
        interest
    }

    #[inline(always)]
    #[must_use]
    pub const fn metadata(&self) -> &'static Metadata<'static> {
        self.metadata
    }
}

impl Callsite for DefaultCallsite {
    fn metadata(&self) -> &Metadata<'_> {
        self.metadata
    }
}

impl fmt::Debug for DefaultCallsite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DefaultCallsite")
            .field("interest", &self.interest)
            .field("meta", &self.metadata)
            .finish()
    }
}

const fn encode(interest: &Interest) -> u32 {
    if interest.is_never() {
        INTEREST_NEVER
    } else if interest.is_always() {
        INTEREST_ALWAYS
    } else {
        INTEREST_SOMETIMES
    }
}

fn decode(code: u32) -> Interest {
    match code {
        INTEREST_NEVER => Interest::never(),
        INTEREST_ALWAYS => Interest::always(),
        INTEREST_SOMETIMES => Interest::sometimes(),
        _ => panic!("a cached interest holds one of the three interest codes"),
    }
}

impl Identifier {
    #[must_use]
    pub const fn new(callsite: &'static dyn Callsite) -> Self {
        Self(callsite)
    }

    fn address(self) -> usize {
        std::ptr::from_ref(self.0).cast::<()>().addr()
    }
}

impl PartialEq for Identifier {
    fn eq(&self, other: &Self) -> bool {
        self.address() == other.address()
    }
}

impl Eq for Identifier {}

impl Hash for Identifier {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.address().hash(state);
    }
}

impl fmt::Debug for Identifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Identifier({:p})",
            std::ptr::from_ref(self.0).cast::<()>()
        )
    }
}
