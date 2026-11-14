use std::{
    collections::{HashMap, HashSet},
    hash::{BuildHasher, Hasher},
};

const FIBONACCI_MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;

#[derive(Clone, Copy, Default)]
pub struct Addresses;

pub type AddressMap<V> = HashMap<usize, V, Addresses>;
pub type AddressSet = HashSet<usize, Addresses>;

pub const fn map<V>() -> AddressMap<V> {
    HashMap::with_hasher(Addresses)
}

pub const fn set() -> AddressSet {
    HashSet::with_hasher(Addresses)
}

impl BuildHasher for Addresses {
    type Hasher = AddressHasher;

    fn build_hasher(&self) -> AddressHasher {
        AddressHasher(0)
    }
}

pub struct AddressHasher(u64);

impl Hasher for AddressHasher {
    fn write(&mut self, bytes: &[u8]) {
        bytes
            .iter()
            .for_each(|&byte| self.write_u64(u64::from(byte)));
    }

    fn write_u64(&mut self, value: u64) {
        let full = u128::from(self.0 ^ value) * u128::from(FIBONACCI_MULTIPLIER);
        self.0 = (full as u64) ^ ((full >> u64::BITS) as u64);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
