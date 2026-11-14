use super::segment::{HEADER_SLICES, SLICE_BYTES, SLICES};

pub const BLOCK_ALIGN_BYTES: usize = 16;
const LINEAR_BYTES_MAX: usize = 128;
const LINEAR_CLASSES: usize = LINEAR_BYTES_MAX / BLOCK_ALIGN_BYTES;
const LINEAR_SHIFT: usize = LINEAR_BYTES_MAX.trailing_zeros() as usize;
const CLASSES_PER_DOUBLING_SHIFT: usize = 2;
const CLASSES_PER_DOUBLING: usize = 1 << CLASSES_PER_DOUBLING_SHIFT;
pub const SMALL_BYTES_MAX: usize = 256 << 10;
pub const CLASSES: usize = class_of(SMALL_BYTES_MAX) + 1;
const PAGE_BLOCKS_MIN: usize = 8;
const DIRECT_BYTES_MAX: usize = 1024;
const DIRECT_STEPS: usize = DIRECT_BYTES_MAX / BLOCK_ALIGN_BYTES + 1;

pub const fn class_of(bytes: usize) -> usize {
    if bytes <= LINEAR_BYTES_MAX {
        return bytes.saturating_sub(1) / BLOCK_ALIGN_BYTES;
    }
    let shift = (usize::BITS - 1 - (bytes - 1).leading_zeros()) as usize;
    let step = 1 << (shift - CLASSES_PER_DOUBLING_SHIFT);
    LINEAR_CLASSES
        + (shift - LINEAR_SHIFT) * CLASSES_PER_DOUBLING
        + ((bytes - 1) - (1 << shift)) / step
}

const fn class_bytes_of(class: usize) -> usize {
    if class < LINEAR_CLASSES {
        return (class + 1) * BLOCK_ALIGN_BYTES;
    }
    let shift = LINEAR_SHIFT + (class - LINEAR_CLASSES) / CLASSES_PER_DOUBLING;
    let index = (class - LINEAR_CLASSES) % CLASSES_PER_DOUBLING;
    (1 << shift) + (index + 1) * (1 << (shift - CLASSES_PER_DOUBLING_SHIFT))
}

const fn slices_of(bytes: usize) -> usize {
    let slices = (bytes * PAGE_BLOCKS_MIN).div_ceil(SLICE_BYTES);
    if slices == 0 { 1 } else { slices }
}

pub static CLASS_BYTES: [usize; CLASSES] = {
    let mut table = [0; CLASSES];
    let mut class = 0;
    while class < CLASSES {
        table[class] = class_bytes_of(class);
        class += 1;
    }
    table
};

pub static CLASS_SLICES: [usize; CLASSES] = {
    let mut table = [0; CLASSES];
    let mut class = 0;
    while class < CLASSES {
        table[class] = slices_of(class_bytes_of(class));
        class += 1;
    }
    table
};

static DIRECT: [u8; DIRECT_STEPS] = {
    let mut table = [0; DIRECT_STEPS];
    let mut step = 0;
    while step < DIRECT_STEPS {
        table[step] = class_of(step * BLOCK_ALIGN_BYTES) as u8;
        step += 1;
    }
    table
};

const _: () = {
    assert!(CLASSES <= u8::MAX as usize);
    assert!(class_bytes_of(CLASSES - 1) == SMALL_BYTES_MAX);
    assert!(slices_of(SMALL_BYTES_MAX) <= SLICES - HEADER_SLICES);
    assert!(slices_of(SMALL_BYTES_MAX) < u64::BITS as usize);
    let mut class = 0;
    while class < CLASSES {
        assert!(class_bytes_of(class).is_multiple_of(BLOCK_ALIGN_BYTES));
        assert!(class_of(class_bytes_of(class)) == class);
        class += 1;
    }
};

#[inline]
pub fn class_for(bytes: usize) -> usize {
    if bytes <= DIRECT_BYTES_MAX {
        return usize::from(DIRECT[bytes.div_ceil(BLOCK_ALIGN_BYTES)]);
    }
    class_of(bytes)
}

#[inline]
pub const fn small_bytes(size: usize, align: usize) -> Option<usize> {
    if align <= BLOCK_ALIGN_BYTES {
        return if size <= SMALL_BYTES_MAX {
            Some(size)
        } else {
            None
        };
    }
    let wide = if size > align { size } else { align };
    if wide > SLICE_BYTES {
        return None;
    }
    Some(wide.next_power_of_two())
}
