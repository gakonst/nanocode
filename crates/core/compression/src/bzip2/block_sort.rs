use core::cmp::Ordering;

use super::{BYTE_VALUES, OVERSHOOT_BYTES};

pub(super) const PAIR_TABLE_ENTRIES: usize = (1 << u16::BITS) + 1;
const FALLBACK_BLOCK_BYTES_LIMIT: usize = 10_000;
const WORK_FACTOR: i32 = 30;
const WORK_FACTOR_DIVISOR: i32 = 3;
const BUDGET_PER_BYTE: i32 = (WORK_FACTOR - 1) / WORK_FACTOR_DIVISOR;
const FALLBACK_SIMPLE_SPAN_LIMIT: i32 = 10;
const FALLBACK_STACK_ENTRIES: usize = 100;
const FALLBACK_INSERTION_STRIDE: usize = 4;
const RANDOM_MULTIPLIER: u32 = 7621;
const RANDOM_MODULUS: u32 = 32_768;
const MEDIAN_CHOICES: u32 = 3;
const BUCKET_WORD_BITS: usize = u32::BITS as usize;
const BUCKET_WORD_SHIFT: usize = 5;
const BUCKET_WORD_FULL: u32 = u32::MAX;
const SENTINEL_PAIRS: usize = 32;
const BUCKET_WORDS_SLACK: usize = 2;
const MAIN_SIMPLE_SPAN_LIMIT: i32 = 20;
const RADIX_DEPTH: u32 = 2;
const QUICK_SORT_DEPTH: u32 = 12;
const MAIN_DEPTH_LIMIT: u32 = RADIX_DEPTH + QUICK_SORT_DEPTH;
const MAIN_STACK_ENTRIES: usize = 100;
const MAIN_STACK_PUSH_ENTRIES: usize = 3;
const COMPARE_PREFIX_BYTES: usize = 12;
const COMPARE_STEP_BYTES: usize = 8;
const SHELL_GAPS: [i32; 14] = [
    1, 4, 13, 40, 121, 364, 1093, 3280, 9841, 29524, 88573, 265_720, 797_161, 2_391_484,
];
const SHELL_GAP_FACTOR: i32 = 3;
const SORTED_FLAG: u32 = 1 << 21;
const POSITION_MASK: u32 = !SORTED_FLAG;
const QUADRANT_SPAN_MAX: i32 = 65_534;
const QUADRANT_BITS: u32 = u16::BITS - 1;

pub(super) struct Workspace<'a> {
    pub(super) block: &'a mut [u8],
    pub(super) quadrant: &'a mut [u16],
    pub(super) order: &'a mut [u32],
    pub(super) pairs: &'a mut [u32],
    pub(super) classes: &'a mut [u32],
}

pub(super) fn sort(workspace: &mut Workspace<'_>, block_bytes: usize) -> usize {
    assert!(block_bytes > 0);
    assert!(workspace.block.len() >= block_bytes + OVERSHOOT_BYTES);
    assert!(workspace.order.len() >= block_bytes);
    if block_bytes < FALLBACK_BLOCK_BYTES_LIMIT {
        fallback_sort(workspace, block_bytes);
    } else {
        let block_bytes_signed = i32::try_from(block_bytes).expect("a block is under 1 MB");
        let mut budget = block_bytes_signed * BUDGET_PER_BYTE;
        main_sort(workspace, block_bytes, &mut budget);
        if budget < 0 {
            fallback_sort(workspace, block_bytes);
        }
    }
    workspace.order[..block_bytes]
        .iter()
        .position(|&position| position == 0)
        .expect("the rotation starting at 0 is in the sorted order")
}

fn fallback_simple_sort(order: &mut [u32], classes: &[u32], low: i32, high: i32) {
    if high <= low {
        return;
    }
    let low = usize::try_from(low).expect("a bucket start is non-negative");
    let high = usize::try_from(high).expect("a bucket end is non-negative");
    if high - low >= FALLBACK_INSERTION_STRIDE {
        insertion_pass(order, classes, low, high, FALLBACK_INSERTION_STRIDE);
    }
    insertion_pass(order, classes, low, high, 1);
}

fn insertion_pass(order: &mut [u32], classes: &[u32], low: usize, high: usize, stride: usize) {
    for index in (low..=high - stride).rev() {
        let value = order[index];
        let class = classes[value as usize];
        let mut next = index + stride;
        while next <= high && class > classes[order[next] as usize] {
            order[next - stride] = order[next];
            next += stride;
        }
        order[next - stride] = value;
    }
}

fn fallback_quick_sort(order: &mut [u32], classes: &[u32], low_start: i32, high_start: i32) {
    let mut stack = [(0_i32, 0_i32); FALLBACK_STACK_ENTRIES];
    let mut depth = 1;
    let mut random = 0_u32;
    stack[0] = (low_start, high_start);
    while depth > 0 {
        assert!(depth < FALLBACK_STACK_ENTRIES - 1);
        depth -= 1;
        let (low, high) = stack[depth];
        if high - low < FALLBACK_SIMPLE_SPAN_LIMIT {
            fallback_simple_sort(order, classes, low, high);
            continue;
        }
        random = random.wrapping_mul(RANDOM_MULTIPLIER).wrapping_add(1) % RANDOM_MODULUS;
        let pivot_index = match random % MEDIAN_CHOICES {
            0 => low,
            1 => (low + high) >> 1,
            _ => high,
        };
        let pivot = classes[order[pivot_index as usize] as usize];
        let (less_end, unsorted_low, unsorted_high, greater_start) =
            fallback_partition(order, classes, low, high, pivot);
        if greater_start < less_end {
            continue;
        }
        let count = (less_end - low).min(unsorted_low - less_end);
        swap_ranges(order, low, unsorted_low - count, count);
        let count = (high - greater_start).min(greater_start - unsorted_high);
        swap_ranges(order, unsorted_low, high - count + 1, count);
        let left_end = low + unsorted_low - less_end - 1;
        let right_start = high - (greater_start - unsorted_high) + 1;
        if left_end - low > high - right_start {
            stack[depth] = (low, left_end);
            stack[depth + 1] = (right_start, high);
        } else {
            stack[depth] = (right_start, high);
            stack[depth + 1] = (low, left_end);
        }
        depth += 2;
    }
}

fn fallback_partition(
    order: &mut [u32],
    classes: &[u32],
    low: i32,
    high: i32,
    pivot: u32,
) -> (i32, i32, i32, i32) {
    let (mut less_end, mut unsorted_low) = (low, low);
    let (mut greater_start, mut unsorted_high) = (high, high);
    loop {
        while unsorted_low <= unsorted_high {
            let class = classes[order[unsorted_low as usize] as usize];
            match class.cmp(&pivot) {
                Ordering::Greater => break,
                Ordering::Equal => {
                    order.swap(unsorted_low as usize, less_end as usize);
                    less_end += 1;
                    unsorted_low += 1;
                }
                Ordering::Less => unsorted_low += 1,
            }
        }
        while unsorted_low <= unsorted_high {
            let class = classes[order[unsorted_high as usize] as usize];
            match class.cmp(&pivot) {
                Ordering::Less => break,
                Ordering::Equal => {
                    order.swap(unsorted_high as usize, greater_start as usize);
                    greater_start -= 1;
                    unsorted_high -= 1;
                }
                Ordering::Greater => unsorted_high -= 1,
            }
        }
        if unsorted_low > unsorted_high {
            return (less_end, unsorted_low, unsorted_high, greater_start);
        }
        order.swap(unsorted_low as usize, unsorted_high as usize);
        unsorted_low += 1;
        unsorted_high -= 1;
    }
}

fn swap_ranges(order: &mut [u32], first: i32, second: i32, count: i32) {
    for offset in 0..count {
        order.swap((first + offset) as usize, (second + offset) as usize);
    }
}

const fn bucket_bit(position: usize) -> (usize, u32) {
    (
        position >> BUCKET_WORD_SHIFT,
        1 << (position % BUCKET_WORD_BITS),
    )
}

const fn set_bucket(heads: &mut [u32], position: usize) {
    let (word, bit) = bucket_bit(position);
    heads[word] |= bit;
}

const fn clear_bucket(heads: &mut [u32], position: usize) {
    let (word, bit) = bucket_bit(position);
    heads[word] &= !bit;
}

const fn is_bucket(heads: &[u32], position: usize) -> bool {
    let (word, bit) = bucket_bit(position);
    heads[word] & bit != 0
}

fn fallback_sort(workspace: &mut Workspace<'_>, block_bytes: usize) {
    let block = &workspace.block[..block_bytes];
    let order = &mut *workspace.order;
    let heads = &mut *workspace.pairs;
    let classes = &mut *workspace.classes;
    let mut counts = [0_usize; BYTE_VALUES + 1];
    for &byte in block {
        counts[usize::from(byte)] += 1;
    }
    for index in 1..=BYTE_VALUES {
        counts[index] += counts[index - 1];
    }
    for (position, &byte) in block.iter().enumerate() {
        let slot = &mut counts[usize::from(byte)];
        *slot -= 1;
        order[*slot] = u32::try_from(position).expect("a block position fits in u32");
    }
    heads[..BUCKET_WORDS_SLACK + block_bytes / BUCKET_WORD_BITS].fill(0);
    for &start in &counts[..BYTE_VALUES] {
        set_bucket(heads, start);
    }
    for pair in 0..SENTINEL_PAIRS {
        set_bucket(heads, block_bytes + 2 * pair);
        clear_bucket(heads, block_bytes + 2 * pair + 1);
    }
    let mut span = 1;
    loop {
        let mut head = 0;
        for (index, &position) in order[..block_bytes].iter().enumerate() {
            if is_bucket(heads, index) {
                head = index;
            }
            let position = position as usize;
            let shifted = if position >= span {
                position - span
            } else {
                position + block_bytes - span
            };
            classes[shifted] = u32::try_from(head).expect("a block position fits in u32");
        }
        let unsorted = refine_buckets(order, heads, classes, block_bytes);
        span *= 2;
        if span > block_bytes || unsorted == 0 {
            return;
        }
    }
}

fn refine_buckets(
    order: &mut [u32],
    heads: &mut [u32],
    classes: &[u32],
    block_bytes: usize,
) -> usize {
    let mut unsorted = 0;
    let mut next = 0;
    loop {
        let start = skip_while_bucket(heads, next, true) - 1;
        if start >= block_bytes {
            return unsorted;
        }
        let end = skip_while_bucket(heads, start + 1, false) - 1;
        if end >= block_bytes {
            return unsorted;
        }
        next = end + 1;
        if end > start {
            unsorted += end - start + 1;
            let low = i32::try_from(start).expect("a block position fits in i32");
            let high = i32::try_from(end).expect("a block position fits in i32");
            fallback_quick_sort(order, classes, low, high);
            let mut class = None;
            for index in start..=end {
                let current = classes[order[index] as usize];
                if class != Some(current) {
                    set_bucket(heads, index);
                    class = Some(current);
                }
            }
        }
    }
}

const fn skip_while_bucket(heads: &[u32], mut position: usize, set: bool) -> usize {
    let word_skip = if set { BUCKET_WORD_FULL } else { 0 };
    while is_bucket(heads, position) == set && !position.is_multiple_of(BUCKET_WORD_BITS) {
        position += 1;
    }
    if is_bucket(heads, position) == set {
        while heads[position >> BUCKET_WORD_SHIFT] == word_skip {
            position += BUCKET_WORD_BITS;
        }
        while is_bucket(heads, position) == set {
            position += 1;
        }
    }
    position
}

struct Rotations<'a> {
    block: &'a [u8],
    quadrant: &'a [u16],
    block_bytes: usize,
}

impl Rotations<'_> {
    fn greater(&self, first: u32, second: u32, budget: &mut i32) -> bool {
        let mut first = first as usize;
        let mut second = second as usize;
        assert_ne!(first, second);
        match self.block[first..first + COMPARE_PREFIX_BYTES]
            .cmp(&self.block[second..second + COMPARE_PREFIX_BYTES])
        {
            Ordering::Greater => return true,
            Ordering::Less => return false,
            Ordering::Equal => {}
        }
        first += COMPARE_PREFIX_BYTES;
        second += COMPARE_PREFIX_BYTES;
        for _ in 0..self.block_bytes.div_ceil(COMPARE_STEP_BYTES) {
            let bytes_first = &self.block[first..first + COMPARE_STEP_BYTES];
            let bytes_second = &self.block[second..second + COMPARE_STEP_BYTES];
            let quadrant_first = &self.quadrant[first..first + COMPARE_STEP_BYTES];
            let quadrant_second = &self.quadrant[second..second + COMPARE_STEP_BYTES];
            if bytes_first != bytes_second || quadrant_first != quadrant_second {
                for offset in 0..COMPARE_STEP_BYTES {
                    if bytes_first[offset] != bytes_second[offset] {
                        return bytes_first[offset] > bytes_second[offset];
                    }
                    if quadrant_first[offset] != quadrant_second[offset] {
                        return quadrant_first[offset] > quadrant_second[offset];
                    }
                }
            }
            first += COMPARE_STEP_BYTES;
            second += COMPARE_STEP_BYTES;
            if first >= self.block_bytes {
                first -= self.block_bytes;
            }
            if second >= self.block_bytes {
                second -= self.block_bytes;
            }
            *budget -= 1;
        }
        false
    }

    const fn byte_at(&self, position: u32, depth: u32) -> u8 {
        self.block[position.wrapping_add(depth) as usize]
    }
}

fn main_simple_sort(
    order: &mut [u32],
    rotations: &Rotations<'_>,
    low: i32,
    high: i32,
    depth: u32,
    budget: &mut i32,
) {
    let count = high - low + 1;
    let Some(gap_count) = SHELL_GAPS.iter().position(|&gap| gap >= count) else {
        return;
    };
    for &gap in SHELL_GAPS[..gap_count].iter().rev() {
        for index in low + gap..=high {
            let value = order[index as usize];
            let mut slot = index;
            while rotations.greater(
                order[(slot - gap) as usize].wrapping_add(depth),
                value.wrapping_add(depth),
                budget,
            ) {
                order[slot as usize] = order[(slot - gap) as usize];
                slot -= gap;
                if slot < low + gap {
                    break;
                }
            }
            order[slot as usize] = value;
            if *budget < 0 {
                return;
            }
        }
    }
}

const fn median_of_three(first: u8, second: u8, third: u8) -> u8 {
    let (low, high) = if first > second {
        (second, first)
    } else {
        (first, second)
    };
    if third <= low {
        low
    } else if third >= high {
        high
    } else {
        third
    }
}

fn main_quick_sort(
    order: &mut [u32],
    rotations: &Rotations<'_>,
    low_start: i32,
    high_start: i32,
    budget: &mut i32,
) {
    let mut stack = [(0_i32, 0_i32, 0_u32); MAIN_STACK_ENTRIES];
    stack[0] = (low_start, high_start, RADIX_DEPTH);
    let mut stack_depth = 1;
    while stack_depth > 0 {
        assert!(stack_depth < MAIN_STACK_ENTRIES - 2);
        stack_depth -= 1;
        let (low, high, depth) = stack[stack_depth];
        if high - low < MAIN_SIMPLE_SPAN_LIMIT || depth > MAIN_DEPTH_LIMIT {
            main_simple_sort(order, rotations, low, high, depth, budget);
            if *budget < 0 {
                return;
            }
            continue;
        }
        let pivot = median_of_three(
            rotations.byte_at(order[low as usize], depth),
            rotations.byte_at(order[high as usize], depth),
            rotations.byte_at(order[((low + high) >> 1) as usize], depth),
        );
        let (less_end, unsorted_low, unsorted_high, greater_start) =
            main_partition(order, rotations, low, high, depth, pivot);
        if greater_start < less_end {
            stack[stack_depth] = (low, high, depth + 1);
            stack_depth += 1;
            continue;
        }
        let count = (less_end - low).min(unsorted_low - less_end);
        swap_ranges(order, low, unsorted_low - count, count);
        let count = (high - greater_start).min(greater_start - unsorted_high);
        swap_ranges(order, unsorted_low, high - count + 1, count);
        let left_end = low + unsorted_low - less_end - 1;
        let right_start = high - (greater_start - unsorted_high) + 1;
        let mut next = [
            (low, left_end, depth),
            (right_start, high, depth),
            (left_end + 1, right_start - 1, depth + 1),
        ];
        let span = |range: (i32, i32, u32)| range.1 - range.0;
        if span(next[0]) < span(next[1]) {
            next.swap(0, 1);
        }
        if span(next[1]) < span(next[2]) {
            next.swap(1, 2);
        }
        if span(next[0]) < span(next[1]) {
            next.swap(0, 1);
        }
        stack[stack_depth..stack_depth + MAIN_STACK_PUSH_ENTRIES].copy_from_slice(&next);
        stack_depth += MAIN_STACK_PUSH_ENTRIES;
    }
}

fn main_partition(
    order: &mut [u32],
    rotations: &Rotations<'_>,
    low: i32,
    high: i32,
    depth: u32,
    pivot: u8,
) -> (i32, i32, i32, i32) {
    let (mut less_end, mut unsorted_low) = (low, low);
    let (mut greater_start, mut unsorted_high) = (high, high);
    loop {
        while unsorted_low <= unsorted_high {
            match rotations
                .byte_at(order[unsorted_low as usize], depth)
                .cmp(&pivot)
            {
                Ordering::Greater => break,
                Ordering::Equal => {
                    order.swap(unsorted_low as usize, less_end as usize);
                    less_end += 1;
                    unsorted_low += 1;
                }
                Ordering::Less => unsorted_low += 1,
            }
        }
        while unsorted_low <= unsorted_high {
            match rotations
                .byte_at(order[unsorted_high as usize], depth)
                .cmp(&pivot)
            {
                Ordering::Less => break,
                Ordering::Equal => {
                    order.swap(unsorted_high as usize, greater_start as usize);
                    greater_start -= 1;
                    unsorted_high -= 1;
                }
                Ordering::Greater => unsorted_high -= 1,
            }
        }
        if unsorted_low > unsorted_high {
            return (less_end, unsorted_low, unsorted_high, greater_start);
        }
        order.swap(unsorted_low as usize, unsorted_high as usize);
        unsorted_low += 1;
        unsorted_high -= 1;
    }
}

fn main_sort(workspace: &mut Workspace<'_>, block_bytes: usize, budget: &mut i32) {
    let pairs = &mut *workspace.pairs;
    let order = &mut *workspace.order;
    let block = &mut *workspace.block;
    workspace.quadrant[..block_bytes + OVERSHOOT_BYTES].fill(0);
    count_pairs(block, pairs, block_bytes);
    block.copy_within(..OVERSHOOT_BYTES, block_bytes);
    for index in 1..PAIR_TABLE_ENTRIES {
        pairs[index] += pairs[index - 1];
    }
    let mut pair = u16::from(block[0]) << u8::BITS;
    for (position, &byte) in block[..block_bytes].iter().enumerate().rev() {
        pair = (pair >> u8::BITS) | (u16::from(byte) << u8::BITS);
        let slot = &mut pairs[usize::from(pair)];
        *slot -= 1;
        order[*slot as usize] = u32::try_from(position).expect("a block position fits in u32");
    }
    let running_order = bucket_order(pairs);
    let quadrant = &mut *workspace.quadrant;
    let mut done = [false; BYTE_VALUES];
    for (rank, &bucket) in running_order.iter().enumerate() {
        let rotations = Rotations {
            block,
            quadrant,
            block_bytes,
        };
        sort_small_buckets(order, pairs, &rotations, bucket, budget);
        if *budget < 0 {
            return;
        }
        assert!(!done[bucket]);
        synthesize_buckets(order, pairs, block, &done, bucket, block_bytes);
        done[bucket] = true;
        if rank < BYTE_VALUES - 1 {
            update_quadrant(order, pairs, quadrant, bucket, block_bytes);
        }
    }
}

fn count_pairs(block: &[u8], pairs: &mut [u32], block_bytes: usize) {
    pairs.fill(0);
    let mut pair = usize::from(block[0]) << u8::BITS;
    for &byte in block[..block_bytes].iter().rev() {
        pair = (pair >> u8::BITS) | (usize::from(byte) << u8::BITS);
        pairs[pair] += 1;
    }
}

const fn bucket_size(pairs: &[u32], bucket: usize) -> u32 {
    pairs[(bucket + 1) << u8::BITS] - pairs[bucket << u8::BITS]
}

fn bucket_order(pairs: &[u32]) -> [usize; BYTE_VALUES] {
    let mut running_order: [usize; BYTE_VALUES] = core::array::from_fn(|bucket| bucket);
    let mut gap = 1;
    while gap <= BYTE_VALUES {
        gap = SHELL_GAP_FACTOR as usize * gap + 1;
    }
    loop {
        gap /= SHELL_GAP_FACTOR as usize;
        for index in gap..BYTE_VALUES {
            let value = running_order[index];
            let mut slot = index;
            while bucket_size(pairs, running_order[slot - gap]) > bucket_size(pairs, value) {
                running_order[slot] = running_order[slot - gap];
                slot -= gap;
                if slot < gap {
                    break;
                }
            }
            running_order[slot] = value;
        }
        if gap == 1 {
            return running_order;
        }
    }
}

fn sort_small_buckets(
    order: &mut [u32],
    pairs: &mut [u32],
    rotations: &Rotations<'_>,
    bucket: usize,
    budget: &mut i32,
) {
    for second in 0..BYTE_VALUES {
        if second == bucket {
            continue;
        }
        let pair = (bucket << u8::BITS) + second;
        if pairs[pair] & SORTED_FLAG == 0 {
            let low = (pairs[pair] & POSITION_MASK) as i32;
            let high = (pairs[pair + 1] & POSITION_MASK).wrapping_sub(1) as i32;
            if high > low {
                main_quick_sort(order, rotations, low, high, budget);
                if *budget < 0 {
                    return;
                }
            }
        }
        pairs[pair] |= SORTED_FLAG;
    }
}

fn synthesize_buckets(
    order: &mut [u32],
    pairs: &mut [u32],
    block: &[u8],
    done: &[bool; BYTE_VALUES],
    bucket: usize,
    block_bytes: usize,
) {
    let mut copy_start = [0_i32; BYTE_VALUES];
    let mut copy_end = [0_i32; BYTE_VALUES];
    for first in 0..BYTE_VALUES {
        copy_start[first] = (pairs[(first << u8::BITS) + bucket] & POSITION_MASK) as i32;
        copy_end[first] = (pairs[(first << u8::BITS) + bucket + 1] & POSITION_MASK) as i32 - 1;
    }
    let preceding = |order: &[u32], index: i32| {
        let position = match order[index as usize] {
            0 => block_bytes,
            position => position as usize,
        };
        position - 1
    };
    let mut index = (pairs[bucket << u8::BITS] & POSITION_MASK) as i32;
    while index < copy_start[bucket] {
        let position = preceding(order, index);
        let byte = usize::from(block[position]);
        if !done[byte] {
            order[copy_start[byte] as usize] = position as u32;
            copy_start[byte] += 1;
        }
        index += 1;
    }
    let mut index = (pairs[(bucket + 1) << u8::BITS] & POSITION_MASK) as i32 - 1;
    while index > copy_end[bucket] {
        let position = preceding(order, index);
        let byte = usize::from(block[position]);
        if !done[byte] {
            order[copy_end[byte] as usize] = position as u32;
            copy_end[byte] -= 1;
        }
        index -= 1;
    }
    let block_end = i32::try_from(block_bytes).expect("a block is under 1 MB") - 1;
    assert!(
        copy_start[bucket] - 1 == copy_end[bucket]
            || (copy_start[bucket] == 0 && copy_end[bucket] == block_end)
    );
    for first in 0..BYTE_VALUES {
        pairs[(first << u8::BITS) + bucket] |= SORTED_FLAG;
    }
}

fn update_quadrant(
    order: &[u32],
    pairs: &[u32],
    quadrant: &mut [u16],
    bucket: usize,
    block_bytes: usize,
) {
    let start = (pairs[bucket << u8::BITS] & POSITION_MASK) as usize;
    let size = (pairs[(bucket + 1) << u8::BITS] & POSITION_MASK) as i32 - start as i32;
    let shift = if size <= QUADRANT_SPAN_MAX {
        0
    } else {
        u32::BITS - 1 - size.leading_zeros() - QUADRANT_BITS
    };
    for index in (0..size).rev() {
        let position = order[start + index as usize] as usize;
        let value = u16::try_from(index >> shift).expect("a quadrant value fits in 16 bits");
        quadrant[position] = value;
        if position < OVERSHOOT_BYTES {
            quadrant[position + block_bytes] = value;
        }
    }
}
