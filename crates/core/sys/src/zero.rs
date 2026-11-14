use std::{
    mem::MaybeUninit,
    ptr,
    sync::atomic::{Ordering, compiler_fence},
};

pub fn zero(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        unsafe { ptr::write_volatile(byte, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

pub fn zero_vec(bytes: &mut Vec<u8>) {
    zero(bytes);
    bytes.clear();
    for byte in bytes.spare_capacity_mut() {
        unsafe { ptr::write_volatile(byte, MaybeUninit::new(0)) };
    }
    compiler_fence(Ordering::SeqCst);
}
