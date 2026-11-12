use crate::rng::Rng;

pub const PROFILE_ROLLS: u64 = 10;
pub const CLEAN_ROLLS_END: u64 = 4;
pub const ERRORS_ROLLS_END: u64 = 6;
pub const ASYNC_ROLLS_END: u64 = 8;

pub const LATENCY_MIN_NS_LOW: u64 = 20_000;
pub const LATENCY_MIN_NS_HIGH: u64 = 120_000;
pub const LATENCY_SPREAD_NS_MAX: u64 = 2_000_000;

pub const FAULT_PPM_MIN: u64 = 1_000;
pub const WRITE_FAULT_PPM_MAX: u64 = 50_000;
pub const READ_FAULT_PPM_MAX: u64 = 20_000;

pub const SYNC_PPM_MIN: u64 = 10_000;
pub const SYNC_PPM_MAX: u64 = 300_000;

pub const CACHE_LOST_PPM_MIN: u64 = 50_000;
pub const CACHE_LOST_PPM_MAX: u64 = 500_000;
pub const CACHE_WRITES: usize = 32;

pub const FAULTY_SHARE_PPM: u64 = 500_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    OnAck,
    Async { sync_ppm: u64 },
    LyingCache { lost_ppm: u64 },
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: &'static str,
    pub before_ppm: u64,
    pub after_ppm: u64,
    pub torn_ppm: u64,
    pub read_ppm: u64,
    pub durability: Durability,
    pub latency_min_ns: u64,
    pub latency_spread_ns: u64,
}

fn kind(rng: &mut Rng) -> (&'static str, bool, Durability) {
    let roll = rng.below(PROFILE_ROLLS);
    if roll < CLEAN_ROLLS_END {
        return ("clean", false, Durability::OnAck);
    }
    if roll < ERRORS_ROLLS_END {
        return ("errors", true, Durability::OnAck);
    }
    let faulty = rng.chance(FAULTY_SHARE_PPM);
    if roll < ASYNC_ROLLS_END {
        let sync_ppm = rng.range(SYNC_PPM_MIN, SYNC_PPM_MAX);
        return ("async", faulty, Durability::Async { sync_ppm });
    }
    let lost_ppm = rng.range(CACHE_LOST_PPM_MIN, CACHE_LOST_PPM_MAX);
    ("lying", faulty, Durability::LyingCache { lost_ppm })
}

impl Profile {
    pub fn of(seed: u64) -> Self {
        let mut rng = Rng::derive(seed, "profile");
        let latency_min_ns = rng.range(LATENCY_MIN_NS_LOW, LATENCY_MIN_NS_HIGH);
        let latency_spread_ns = rng.range(0, LATENCY_SPREAD_NS_MAX);
        let (name, faulty, durability) = kind(&mut rng);
        let mut ppm = |max: u64| {
            if faulty {
                rng.range(FAULT_PPM_MIN, max)
            } else {
                0
            }
        };
        Self {
            name,
            before_ppm: ppm(WRITE_FAULT_PPM_MAX),
            after_ppm: ppm(WRITE_FAULT_PPM_MAX),
            torn_ppm: ppm(WRITE_FAULT_PPM_MAX),
            read_ppm: ppm(READ_FAULT_PPM_MAX),
            durability,
            latency_min_ns,
            latency_spread_ns,
        }
    }
}
