#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LockKey {
    pub class: i32,
    pub id: i32,
    pub shared: bool,
}

impl LockKey {
    #[must_use]
    pub fn named(class: i32, name: &str, shared: bool) -> Self {
        let hash = name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
        Self {
            class,
            id: i32::from_ne_bytes(hash.to_ne_bytes()),
            shared,
        }
    }

    #[must_use]
    pub fn merged(requested: impl IntoIterator<Item = Self>) -> Vec<Self> {
        let mut keys: Vec<Self> = requested.into_iter().collect();
        keys.sort_unstable();
        keys.dedup_by(|later, earlier| {
            let same = (later.class, later.id) == (earlier.class, earlier.id);
            if same {
                earlier.shared &= later.shared;
            }
            same
        });
        keys
    }
}

#[derive(Clone, Debug)]
pub enum LocksHeld {
    Exclusive,
    Keys(Vec<LockKey>),
}

impl LocksHeld {
    #[must_use]
    pub fn covers(&self, key: LockKey) -> bool {
        match self {
            Self::Exclusive => true,
            Self::Keys(keys) => keys.iter().any(|held| {
                (held.class, held.id) == (key.class, key.id) && (key.shared || !held.shared)
            }),
        }
    }
}
