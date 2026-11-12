use super::bits::Input;
use super::output::Output;
use super::table::{
    CODE_LENGTH_CODES, CODE_LENGTH_ROOT_BITS, CodeKind, DISTANCE_ROOT_BITS, END_OF_BLOCK,
    FIXED_DISTANCE_ROOT_BITS, FIXED_LENGTH_ROOT_BITS, LENGTH_ROOT_BITS, build,
};
use super::{DistanceSource, Inflate, LengthSource, Mode, Step, Stop, TableSizes};

const BLOCK_HEADER_BITS: u32 = 3;
const LAST_BLOCK_BITS: u32 = 1;
const BLOCK_TYPE_BITS: u32 = 2;
const BLOCK_STORED: usize = 0;
const BLOCK_FIXED: usize = 1;
const BLOCK_DYNAMIC: usize = 2;
const STORED_LENGTHS_BITS: u32 = 32;
const STORED_LENGTH_MASK: u32 = 0xFFFF;
const STORED_LENGTH_BITS: u32 = 16;
const LENGTH_COUNT_BITS: u32 = 5;
const LENGTH_COUNT_MIN: usize = 257;
const LENGTH_COUNT_MAX: usize = 286;
const DISTANCE_COUNT_BITS: u32 = 5;
const DISTANCE_COUNT_MIN: usize = 1;
const DISTANCE_COUNT_MAX: usize = 30;
const CODE_LENGTH_COUNT_BITS: u32 = 4;
const CODE_LENGTH_COUNT_MIN: usize = 4;
const TABLE_SIZES_BITS: u32 = LENGTH_COUNT_BITS + DISTANCE_COUNT_BITS + CODE_LENGTH_COUNT_BITS;
const CODE_LENGTH_BITS: u32 = 3;
const CODE_LENGTH_ORDER: [u8; CODE_LENGTH_CODES] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];
const CODE_LENGTH_LITERAL_MAX: u16 = 15;
const REPEAT_PREVIOUS: u16 = 16;
const REPEAT_ZERO_SHORT: u16 = 17;
const REPEAT_PREVIOUS_BITS: u32 = 2;
const REPEAT_PREVIOUS_MIN: usize = 3;
const REPEAT_ZERO_SHORT_BITS: u32 = 3;
const REPEAT_ZERO_SHORT_MIN: usize = 3;
const REPEAT_ZERO_LONG_BITS: u32 = 7;
const REPEAT_ZERO_LONG_MIN: usize = 11;

impl Inflate {
    pub(super) fn block_header(&mut self, input: &mut Input<'_>) -> Step {
        if self.last_block {
            input.byte_boundary();
            self.mode = Mode::Check;
            return Ok(());
        }
        input.need(BLOCK_HEADER_BITS)?;
        self.last_block = input.peek(LAST_BLOCK_BITS) != 0;
        input.consume(LAST_BLOCK_BITS);
        let kind = input.peek(BLOCK_TYPE_BITS);
        input.consume(BLOCK_TYPE_BITS);
        self.mode = match kind {
            BLOCK_STORED => Mode::StoredLengths,
            BLOCK_FIXED => {
                self.length_source = LengthSource::Fixed;
                self.length_bits = root_bits(FIXED_LENGTH_ROOT_BITS);
                self.distance_source = DistanceSource::Fixed;
                self.distance_bits = root_bits(FIXED_DISTANCE_ROOT_BITS);
                Mode::Length
            }
            BLOCK_DYNAMIC => Mode::TableSizes,
            _ => return Err(self.fail("invalid block type")),
        };
        Ok(())
    }

    pub(super) fn stored_lengths(&mut self, input: &mut Input<'_>) -> Step {
        input.byte_boundary();
        input.need(STORED_LENGTHS_BITS)?;
        let word = input.low_word();
        let length = word & STORED_LENGTH_MASK;
        if length != (word >> STORED_LENGTH_BITS) ^ STORED_LENGTH_MASK {
            return Err(self.fail("invalid stored block lengths"));
        }
        input.clear();
        self.mode = Mode::StoredCopy {
            remaining: usize::try_from(length).expect("a 16-bit length fits in usize"),
        };
        Ok(())
    }

    pub(super) fn stored_copy(
        &mut self,
        input: &mut Input<'_>,
        output: &mut Output<'_>,
        remaining: usize,
    ) -> Step {
        let mut remaining = remaining;
        while remaining != 0 {
            let count = remaining.min(output.remaining()).min(input.remaining());
            if count == 0 {
                self.mode = Mode::StoredCopy { remaining };
                return Err(Stop::Suspend);
            }
            output.extend(&input.rest()[..count]);
            input.advance(count);
            remaining -= count;
        }
        self.mode = Mode::BlockHeader;
        Ok(())
    }

    pub(super) fn table_sizes(&mut self, input: &mut Input<'_>) -> Step {
        input.need(TABLE_SIZES_BITS)?;
        let lengths = input.peek(LENGTH_COUNT_BITS) + LENGTH_COUNT_MIN;
        input.consume(LENGTH_COUNT_BITS);
        let distances = input.peek(DISTANCE_COUNT_BITS) + DISTANCE_COUNT_MIN;
        input.consume(DISTANCE_COUNT_BITS);
        let code_lengths = input.peek(CODE_LENGTH_COUNT_BITS) + CODE_LENGTH_COUNT_MIN;
        input.consume(CODE_LENGTH_COUNT_BITS);
        if lengths > LENGTH_COUNT_MAX || distances > DISTANCE_COUNT_MAX {
            return Err(self.fail("too many length or distance symbols"));
        }
        let sizes = TableSizes {
            lengths,
            distances,
            code_lengths,
        };
        self.mode = Mode::CodeLengthCodes { sizes, have: 0 };
        Ok(())
    }

    pub(super) fn code_length_codes(
        &mut self,
        input: &mut Input<'_>,
        sizes: TableSizes,
        have: usize,
    ) -> Step {
        let mut have = have;
        while have < sizes.code_lengths {
            if let Err(stop) = input.need(CODE_LENGTH_BITS) {
                self.mode = Mode::CodeLengthCodes { sizes, have };
                return Err(stop);
            }
            self.tables.lens[usize::from(CODE_LENGTH_ORDER[have])] =
                code_length(input.peek(CODE_LENGTH_BITS));
            have += 1;
            input.consume(CODE_LENGTH_BITS);
        }
        while have < CODE_LENGTH_CODES {
            self.tables.lens[usize::from(CODE_LENGTH_ORDER[have])] = 0;
            have += 1;
        }
        let tables = &mut *self.tables;
        let Some(root) = build(
            CodeKind::CodeLengths,
            &tables.lens[..CODE_LENGTH_CODES],
            &mut tables.code_lengths,
            CODE_LENGTH_ROOT_BITS,
            &mut tables.work,
        ) else {
            return Err(self.fail("invalid code lengths set"));
        };
        self.length_source = LengthSource::CodeLengths;
        self.length_bits = root_bits(root);
        self.mode = Mode::CodeLengths { sizes, have: 0 };
        Ok(())
    }

    pub(super) fn code_lengths(
        &mut self,
        input: &mut Input<'_>,
        sizes: TableSizes,
        have: usize,
    ) -> Step {
        let total = sizes.lengths + sizes.distances;
        let mut have = have;
        while have < total {
            match self.code_length_symbol(input, have, total) {
                Ok(next) => have = next,
                Err(Stop::Suspend) => {
                    self.mode = Mode::CodeLengths { sizes, have };
                    return Err(Stop::Suspend);
                }
                Err(stop) => return Err(stop),
            }
        }
        self.dynamic_tables(sizes)
    }

    fn code_length_symbol(
        &mut self,
        input: &mut Input<'_>,
        have: usize,
        total: usize,
    ) -> Result<usize, Stop> {
        let here = super::decode::lookup(input, self.length_codes(), self.length_bits)?;
        let bits = u32::from(here.bits);
        let (extra_bits, count_min, value) = match here.value {
            0..=CODE_LENGTH_LITERAL_MAX => {
                input.consume(bits);
                self.tables.lens[have] = here.value;
                return Ok(have + 1);
            }
            REPEAT_PREVIOUS => (REPEAT_PREVIOUS_BITS, REPEAT_PREVIOUS_MIN, None),
            REPEAT_ZERO_SHORT => (REPEAT_ZERO_SHORT_BITS, REPEAT_ZERO_SHORT_MIN, Some(0)),
            _ => (REPEAT_ZERO_LONG_BITS, REPEAT_ZERO_LONG_MIN, Some(0)),
        };
        input.need(bits + extra_bits)?;
        input.consume(bits);
        let value = match value {
            Some(value) => value,
            None if have == 0 => return Err(self.fail("invalid bit length repeat")),
            None => self.tables.lens[have - 1],
        };
        let count = count_min + input.peek(extra_bits);
        input.consume(extra_bits);
        if have + count > total {
            return Err(self.fail("invalid bit length repeat"));
        }
        self.tables.lens[have..have + count].fill(value);
        Ok(have + count)
    }

    fn dynamic_tables(&mut self, sizes: TableSizes) -> Step {
        if self.tables.lens[END_OF_BLOCK] == 0 {
            return Err(self.fail("invalid code -- missing end-of-block"));
        }
        let tables = &mut *self.tables;
        let Some(root) = build(
            CodeKind::Lengths,
            &tables.lens[..sizes.lengths],
            &mut tables.lengths,
            LENGTH_ROOT_BITS,
            &mut tables.work,
        ) else {
            return Err(self.fail("invalid literal/lengths set"));
        };
        self.length_source = LengthSource::Dynamic;
        self.length_bits = root_bits(root);
        let tables = &mut *self.tables;
        let Some(root) = build(
            CodeKind::Distances,
            &tables.lens[sizes.lengths..sizes.lengths + sizes.distances],
            &mut tables.distances,
            DISTANCE_ROOT_BITS,
            &mut tables.work,
        ) else {
            return Err(self.fail("invalid distances set"));
        };
        self.distance_source = DistanceSource::Dynamic;
        self.distance_bits = root_bits(root);
        self.mode = Mode::Length;
        Ok(())
    }
}

fn root_bits(root: usize) -> u32 {
    u32::try_from(root).expect("a table root of at most 15 bits fits in u32")
}

fn code_length(value: usize) -> u16 {
    u16::try_from(value).expect("a 3-bit code length fits in u16")
}
