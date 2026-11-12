use super::super::lzma_common::{BIT_MODEL_TOTAL, BIT_MODEL_TOTAL_BITS};

pub(super) const MOVE_REDUCING_BITS: u32 = 4;
pub(super) const BIT_PRICE_SHIFT_BITS: u32 = 4;
pub(super) const INFINITY_PRICE: u32 = 1 << 30;
const PRICES_ENTRIES: usize = (BIT_MODEL_TOTAL >> MOVE_REDUCING_BITS) as usize;
const PRICE_WIDTH_LIMIT: u32 = 1 << 16;
const PRICE_TABLE_BIAS: u32 = 15;
const FASTPOS_BITS: u32 = 13;
const FASTPOS_ENTRIES: usize = 1 << FASTPOS_BITS;
const FASTPOS_DIRECT_SLOTS: u8 = 2;
const FASTPOS_SLOTS: u8 = (FASTPOS_BITS * 2) as u8;
const FASTPOS_SHIFT_BITS: u32 = FASTPOS_BITS - 1;
const FASTPOS_SLOT_STEP: u32 = 2 * FASTPOS_SHIFT_BITS;

pub(super) static PRICES: [u8; PRICES_ENTRIES] = price_table();
static FASTPOS: [u8; FASTPOS_ENTRIES] = fastpos_table();

const fn price_table() -> [u8; PRICES_ENTRIES] {
    let mut table = [0; PRICES_ENTRIES];
    let step = 1 << MOVE_REDUCING_BITS;
    let mut probability = step / 2;
    while probability < BIT_MODEL_TOTAL {
        let mut width = probability;
        let mut bit_count = 0;
        let mut cycle = 0;
        while cycle < BIT_PRICE_SHIFT_BITS {
            width *= width;
            bit_count <<= 1;
            while width >= PRICE_WIDTH_LIMIT {
                width >>= 1;
                bit_count += 1;
            }
            cycle += 1;
        }
        table[(probability >> MOVE_REDUCING_BITS) as usize] =
            ((BIT_MODEL_TOTAL_BITS << BIT_PRICE_SHIFT_BITS) - PRICE_TABLE_BIAS - bit_count) as u8;
        probability += step;
    }
    table
}

const fn fastpos_table() -> [u8; FASTPOS_ENTRIES] {
    let mut table = [0; FASTPOS_ENTRIES];
    let mut index = 0;
    while index < FASTPOS_DIRECT_SLOTS as usize {
        table[index] = index as u8;
        index += 1;
    }
    let mut slot = FASTPOS_DIRECT_SLOTS;
    while slot < FASTPOS_SLOTS {
        let count = 1_usize << ((slot >> 1) - 1);
        let mut copied = 0;
        while copied < count {
            table[index] = slot;
            index += 1;
            copied += 1;
        }
        slot += 1;
    }
    assert!(index == FASTPOS_ENTRIES);
    table
}

pub(super) fn dist_slot(dist: u32) -> u32 {
    if dist < 1 << FASTPOS_BITS {
        u32::from(FASTPOS[dist as usize])
    } else if dist < 1 << (FASTPOS_BITS + FASTPOS_SHIFT_BITS) {
        u32::from(FASTPOS[(dist >> FASTPOS_SHIFT_BITS) as usize]) + FASTPOS_SLOT_STEP
    } else {
        u32::from(FASTPOS[(dist >> (2 * FASTPOS_SHIFT_BITS)) as usize]) + 2 * FASTPOS_SLOT_STEP
    }
}

pub(super) fn bit_price(probability: u16, bit: u32) -> u32 {
    let mask = 0_u32.wrapping_sub(bit) & (BIT_MODEL_TOTAL - 1);
    u32::from(PRICES[((u32::from(probability) ^ mask) >> MOVE_REDUCING_BITS) as usize])
}

pub(super) fn bit_0_price(probability: u16) -> u32 {
    u32::from(PRICES[usize::from(probability >> MOVE_REDUCING_BITS)])
}

pub(super) fn bit_1_price(probability: u16) -> u32 {
    let flipped = u32::from(probability) ^ (BIT_MODEL_TOTAL - 1);
    u32::from(PRICES[(flipped >> MOVE_REDUCING_BITS) as usize])
}

pub(super) fn bittree_price(
    probabilities: &[u16],
    base: usize,
    bit_levels: u32,
    symbol: u32,
) -> u32 {
    let mut price = 0;
    let mut symbol = symbol + (1 << bit_levels);
    loop {
        let bit = symbol & 1;
        symbol >>= 1;
        price += bit_price(probabilities[base + symbol as usize], bit);
        if symbol == 1 {
            return price;
        }
    }
}

pub(super) fn bittree_reverse_price(
    probabilities: &[u16],
    base: usize,
    bit_levels: u32,
    symbol: u32,
) -> u32 {
    let mut price = 0;
    let mut symbol = symbol;
    let mut model_index = 1;
    for _ in 0..bit_levels {
        let bit = symbol & 1;
        symbol >>= 1;
        price += bit_price(probabilities[base + model_index], bit);
        model_index = (model_index << 1) + bit as usize;
    }
    price
}

pub(super) const fn direct_price(bits: u32) -> u32 {
    bits << BIT_PRICE_SHIFT_BITS
}
