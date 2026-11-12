use super::bits::Input;
use super::decode::{FAST_INPUT_BYTES_MIN, FAST_OUTPUT_BYTES_MIN};
use super::output::Output;
use super::table::{CODE_BITS_MAX, Code, OP_BASE, OP_END_OF_BLOCK, OP_EXTRA_BITS_MASK, OP_LITERAL};
use super::window::Window;

const LITERALS_BEFORE_SYMBOL: usize = 2;
const DISTANCE_EXTRA_BITS_MAX: u32 = 13;
const DISTANCE_REFILL_BITS: u32 = CODE_BITS_MAX as u32 + DISTANCE_EXTRA_BITS_MAX;

pub(super) enum FastEnd {
    Length,
    BlockEnd,
    Invalid,
}

pub(super) struct Codes<'a> {
    lengths: &'a [Code],
    length_mask: u64,
    distances: &'a [Code],
    distance_mask: u64,
}

impl<'a> Codes<'a> {
    pub(super) const fn new(
        lengths: &'a [Code],
        length_bits: u32,
        distances: &'a [Code],
        distance_bits: u32,
    ) -> Self {
        Self {
            lengths,
            length_mask: (1 << length_bits) - 1,
            distances,
            distance_mask: (1 << distance_bits) - 1,
        }
    }

    fn length(&self, hold: u64) -> Code {
        self.lengths[index(hold & self.length_mask)]
    }

    fn distance(&self, hold: u64) -> Code {
        self.distances[index(hold & self.distance_mask)]
    }
}

fn index(value: u64) -> usize {
    usize::try_from(value).expect("a table index of at most 10 bits fits in usize")
}

fn literal(here: Code) -> u8 {
    u8::try_from(here.value).expect("a literal code holds a byte")
}

pub(super) fn decode(
    input: &mut Input<'_>,
    output: &mut Output<'_>,
    codes: &Codes<'_>,
    window: &Window,
) -> FastEnd {
    assert!(input.remaining() >= FAST_INPUT_BYTES_MIN);
    let end = loop {
        input.refill();
        let mut here = codes.length(input.hold());
        for _ in 0..LITERALS_BEFORE_SYMBOL {
            if here.op != OP_LITERAL {
                break;
            }
            output.push(literal(here));
            input.consume(u32::from(here.bits));
            here = codes.length(input.hold());
        }
        if let Some(end) = symbol(input, output, codes, window, here) {
            break end;
        }
        if input.remaining_with_buffer() < FAST_INPUT_BYTES_MIN
            || output.remaining() < FAST_OUTPUT_BYTES_MIN
        {
            break FastEnd::Length;
        }
    };
    input.return_unused();
    end
}

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
    window.copy_to(output, back, from_window);
    if from_window < length {
        output.copy_match(distance, length - from_window);
    }
    true
}
