use super::bits::Input;
use super::fast::{self, Codes, FastEnd};
use super::output::Output;
use super::table::{Code, OP_END_OF_BLOCK, OP_EXTRA_BITS_MASK, OP_INVALID, OP_LITERAL};
use super::{Inflate, Mode, Step, Stop};

pub(super) const FAST_INPUT_BYTES_MIN: usize = 15;
pub(super) const FAST_OUTPUT_BYTES_MIN: usize = 260;

pub(super) fn lookup(input: &mut Input<'_>, table: &[Code], root_bits: u32) -> Result<Code, Stop> {
    loop {
        let here = table[input.peek(root_bits)];
        if u32::from(here.bits) <= input.bits() {
            return Ok(here);
        }
        input.pull_byte()?;
    }
}

fn decode(input: &mut Input<'_>, table: &[Code], root_bits: u32) -> Result<Code, Stop> {
    let mut here = lookup(input, table, root_bits)?;
    if here.is_subtable() {
        let last = here;
        let last_bits = u32::from(last.bits);
        here = loop {
            let index = input.peek(last_bits + u32::from(last.op)) >> last_bits;
            let here = table[usize::from(last.value) + index];
            if last_bits + u32::from(here.bits) <= input.bits() {
                break here;
            }
            input.pull_byte()?;
        };
        input.consume(last_bits);
    }
    input.consume(u32::from(here.bits));
    Ok(here)
}

impl Inflate {
    pub(super) fn length(&mut self, input: &mut Input<'_>, output: &mut Output<'_>) -> Step {
        if input.remaining() >= FAST_INPUT_BYTES_MIN && output.remaining() >= FAST_OUTPUT_BYTES_MIN
        {
            let codes = Codes::new(
                self.length_codes(),
                self.length_bits,
                self.distance_codes(),
                self.distance_bits,
            );
            let end = fast::decode(input, output, &codes, &self.window);
            self.mode = match end {
                FastEnd::Length => Mode::Length,
                FastEnd::BlockEnd => Mode::BlockHeader,
                FastEnd::Invalid => Mode::Bad,
            };
            return Ok(());
        }
        let here = decode(input, self.length_codes(), self.length_bits)?;
        self.mode = if here.op == OP_LITERAL {
            Mode::Literal {
                byte: u8::try_from(here.value).expect("a literal code holds a byte"),
            }
        } else if here.op & OP_END_OF_BLOCK != 0 {
            Mode::BlockHeader
        } else if here.op & OP_INVALID != 0 {
            return Err(self.fail("invalid literal/length code"));
        } else {
            Mode::LengthExtra {
                length: usize::from(here.value),
                extra: u32::from(here.op & OP_EXTRA_BITS_MASK),
            }
        };
        Ok(())
    }

    pub(super) const fn literal(&mut self, output: &mut Output<'_>, byte: u8) -> Step {
        if output.is_full() {
            return Err(Stop::Suspend);
        }
        output.push(byte);
        self.mode = Mode::Length;
        Ok(())
    }

    pub(super) fn length_extra(
        &mut self,
        input: &mut Input<'_>,
        length: usize,
        extra: u32,
    ) -> Step {
        let mut length = length;
        if extra != 0 {
            input.need(extra)?;
            length += input.peek(extra);
            input.consume(extra);
        }
        self.mode = Mode::Distance { length };
        Ok(())
    }

    pub(super) fn distance(&mut self, input: &mut Input<'_>, length: usize) -> Step {
        let here = decode(input, self.distance_codes(), self.distance_bits)?;
        if here.op & OP_INVALID != 0 {
            return Err(self.fail("invalid distance code"));
        }
        self.mode = Mode::DistanceExtra {
            length,
            distance: usize::from(here.value),
            extra: u32::from(here.op & OP_EXTRA_BITS_MASK),
        };
        Ok(())
    }

    pub(super) fn distance_extra(
        &mut self,
        input: &mut Input<'_>,
        length: usize,
        distance: usize,
        extra: u32,
    ) -> Step {
        let mut distance = distance;
        if extra != 0 {
            input.need(extra)?;
            distance += input.peek(extra);
            input.consume(extra);
        }
        self.mode = Mode::Match { length, distance };
        Ok(())
    }

    pub(super) fn copy(&mut self, output: &mut Output<'_>, length: usize, distance: usize) -> Step {
        let mut length = length;
        loop {
            if output.is_full() {
                self.mode = Mode::Match { length, distance };
                return Err(Stop::Suspend);
            }
            let left = output.remaining();
            let written = output.len();
            let count = if distance > written {
                let back = distance - written;
                if back > self.window.have() {
                    return Err(self.fail("invalid distance too far back"));
                }
                let count = back.min(length).min(left);
                self.window.copy_to(output, back, count);
                count
            } else {
                let count = length.min(left);
                output.copy_match(distance, count);
                count
            };
            length -= count;
            if length == 0 {
                self.mode = Mode::Length;
                return Ok(());
            }
        }
    }
}
