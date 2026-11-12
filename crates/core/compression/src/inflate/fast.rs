use super::bits::Input;
use super::decode::{FAST_INPUT_BYTES_MIN, FAST_OUTPUT_BYTES_MIN};
use super::output::Output;
use super::table::{
    CODE_BITS_MAX, Code, LITERAL_ENTRIES, LITERAL_TABLE_BITS, LITERALS_PER_ENTRY, Literals,
    OP_BASE, OP_END_OF_BLOCK, OP_EXTRA_BITS_MASK, OP_LITERAL,
};
use super::window::Window;

const LITERALS_BEFORE_SYMBOL: usize = 2;
const SYMBOLS_PER_ITERATION: usize = LITERALS_BEFORE_SYMBOL + 1;
const LENGTH_EXTRA_BITS_MAX: u32 = 5;
const DISTANCE_EXTRA_BITS_MAX: u32 = 13;
const DISTANCE_REFILL_BITS: u32 = CODE_BITS_MAX as u32 + DISTANCE_EXTRA_BITS_MAX;
const MATCH_BYTES_MAX: usize = 258;
const LITERAL_ENTRIES_PER_REFILL: usize = 3;
const BATCH_BITS_MAX: u32 = LITERAL_ENTRIES_PER_REFILL as u32 * LITERAL_TABLE_BITS as u32
    + CODE_BITS_MAX as u32
    + LENGTH_EXTRA_BITS_MAX
    + DISTANCE_REFILL_BITS;
const BATCH_INPUT_BYTES: usize = BATCH_BITS_MAX.div_ceil(u8::BITS) as usize;
const BATCH_OUTPUT_BYTES: usize = LITERAL_ENTRIES_PER_REFILL * LITERALS_PER_ENTRY + MATCH_BYTES_MAX;

pub(super) enum FastEnd {
    Length,
    BlockEnd,
    Invalid,
}

pub(super) struct Codes<'a> {
    lengths: &'a [Code],
    length_mask: u64,
    literals: &'a [Option<Literals>; LITERAL_ENTRIES],
    distances: &'a [Code],
    distance_mask: u64,
}

impl<'a> Codes<'a> {
    #[inline(always)]
    pub(super) const fn new(
        lengths: &'a [Code],
        length_bits: u32,
        literals: &'a [Option<Literals>; LITERAL_ENTRIES],
        distances: &'a [Code],
        distance_bits: u32,
    ) -> Self {
        Self {
            lengths,
            length_mask: (1 << length_bits) - 1,
            literals,
            distances,
            distance_mask: (1 << distance_bits) - 1,
        }
    }

    #[inline(always)]
    fn length(&self, hold: u64) -> Code {
        self.lengths[index(hold & self.length_mask)]
    }

    #[inline(always)]
    fn literals(&self, hold: u64) -> Option<Literals> {
        self.literals[index(hold % LITERAL_ENTRIES as u64)]
    }

    #[inline(always)]
    fn distance(&self, hold: u64) -> Code {
        self.distances[index(hold & self.distance_mask)]
    }
}

#[inline(always)]
fn index(value: u64) -> usize {
    usize::try_from(value).expect("a table index of at most 12 bits fits in usize")
}

#[inline(always)]
fn literal(here: Code) -> u8 {
    u8::try_from(here.value).expect("a literal code holds a byte")
}

pub(super) fn decode(
    outer_input: &mut Input<'_>,
    outer_output: &mut Output<'_>,
    codes: &Codes<'_>,
    window: &Window,
) -> FastEnd {
    assert!(outer_input.remaining() >= FAST_INPUT_BYTES_MIN);
    assert!(outer_output.remaining() >= FAST_OUTPUT_BYTES_MIN);
    let mut input = *outer_input;
    let mut output = outer_output.reborrow();
    let end = run(&mut input, &mut output, codes, window);
    input.return_unused();
    let filled = output.len();
    *outer_input = input;
    outer_output.resume(filled);
    end
}

#[inline(always)]
fn run(
    input: &mut Input<'_>,
    output: &mut Output<'_>,
    codes: &Codes<'_>,
    window: &Window,
) -> FastEnd {
    let mut boundary = output.len();
    while input.remaining_with_buffer() >= FAST_INPUT_BYTES_MIN + BATCH_INPUT_BYTES
        && output.remaining() >= FAST_OUTPUT_BYTES_MIN + BATCH_OUTPUT_BYTES
    {
        if let Some(end) = batch(input, output, codes, window, &mut boundary) {
            return end;
        }
    }
    let mut leading = LITERALS_BEFORE_SYMBOL - (output.len() - boundary) % SYMBOLS_PER_ITERATION;
    loop {
        input.refill();
        let mut here = codes.length(input.hold());
        for _ in 0..leading {
            if here.op != OP_LITERAL {
                break;
            }
            output.push(literal(here));
            input.consume(u32::from(here.bits));
            here = codes.length(input.hold());
        }
        if let Some(end) = symbol(input, output, codes, window, here) {
            return end;
        }
        if input.remaining_with_buffer() < FAST_INPUT_BYTES_MIN
            || output.remaining() < FAST_OUTPUT_BYTES_MIN
        {
            return FastEnd::Length;
        }
        leading = LITERALS_BEFORE_SYMBOL;
    }
}

#[inline(always)]
fn batch(
    input: &mut Input<'_>,
    output: &mut Output<'_>,
    codes: &Codes<'_>,
    window: &Window,
    boundary: &mut usize,
) -> Option<FastEnd> {
    input.refill();
    for _ in 0..LITERAL_ENTRIES_PER_REFILL {
        let Some(literals) = codes.literals(input.hold()) else {
            let here = codes.length(input.hold());
            let end = symbol(input, output, codes, window, here);
            *boundary = output.len();
            return end;
        };
        output.push_literals(literals.bytes, literals.count());
        input.consume(literals.bits());
    }
    None
}

#[inline(always)]
fn resolve(input: &mut Input<'_>, table: &[Code], here: Code) -> Code {
    input.consume(u32::from(here.bits));
    if !here.is_subtable() {
        return here;
    }
    let next = table[usize::from(here.value) + input.peek(u32::from(here.op))];
    input.consume(u32::from(next.bits));
    assert!(!next.is_subtable());
    next
}

#[inline(always)]
fn symbol(
    input: &mut Input<'_>,
    output: &mut Output<'_>,
    codes: &Codes<'_>,
    window: &Window,
    here: Code,
) -> Option<FastEnd> {
    let here = resolve(input, codes.lengths, here);
    if here.op == OP_LITERAL {
        output.push(literal(here));
        return None;
    }
    if here.op & OP_BASE == 0 {
        return Some(if here.op & OP_END_OF_BLOCK != 0 {
            FastEnd::BlockEnd
        } else {
            FastEnd::Invalid
        });
    }
    let extra = u32::from(here.op & OP_EXTRA_BITS_MASK);
    let length = usize::from(here.value) + input.peek(extra);
    input.consume(extra);
    let here = codes.distance(input.hold());
    if input.bits() < DISTANCE_REFILL_BITS {
        input.refill();
    }
    let here = resolve(input, codes.distances, here);
    if here.op & OP_BASE == 0 {
        return Some(FastEnd::Invalid);
    }
    let extra = u32::from(here.op & OP_EXTRA_BITS_MASK);
    let distance = usize::from(here.value) + input.peek(extra);
    input.consume(extra);
    if copy(output, window, distance, length) {
        None
    } else {
        Some(FastEnd::Invalid)
    }
}

#[inline(always)]
fn copy(output: &mut Output<'_>, window: &Window, distance: usize, length: usize) -> bool {
    let written = output.len();
    if distance <= written {
        output.copy_match_wide(distance, length);
        return true;
    }
    let back = distance - written;
    if back > window.have() {
        return false;
    }
    let from_window = back.min(length);
    let (end_part, start_part) = window.source(back, from_window);
    output.extend(end_part);
    output.extend(start_part);
    if from_window < length {
        output.copy_match(distance, length - from_window);
    }
    true
}
