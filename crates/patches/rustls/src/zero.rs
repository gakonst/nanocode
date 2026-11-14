use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};

pub(crate) trait Zeroize {
    fn zeroize(&mut self);
}

impl<const N: usize> Zeroize for [u8; N] {
    fn zeroize(&mut self) {
        nanocode_sys::zero(self);
    }
}

impl Zeroize for Vec<u8> {
    fn zeroize(&mut self) {
        nanocode_sys::zero_vec(self);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Zeroizing<Z: Zeroize>(Z);

impl<Z: Zeroize> Zeroizing<Z> {
    pub(crate) fn new(value: Z) -> Self {
        Self(value)
    }
}

impl<Z: Zeroize> Deref for Zeroizing<Z> {
    type Target = Z;

    fn deref(&self) -> &Z {
        &self.0
    }
}

impl<Z: Zeroize> DerefMut for Zeroizing<Z> {
    fn deref_mut(&mut self) -> &mut Z {
        &mut self.0
    }
}

impl<Z: Zeroize> Drop for Zeroizing<Z> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
