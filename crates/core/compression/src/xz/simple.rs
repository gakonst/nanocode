use alloc::boxed::Box;

use super::coder::{Code, Coder, buffer_copy};
use super::options::BcjArchitecture;
use super::stream::{Action, Error, Status};

mod arm;
mod arm64;
mod armthumb;
mod ia64;
mod powerpc;
mod riscv;
mod sparc;
mod x86;

const BUFFERED_UNFILTERED_LIMITS: usize = 2;
const BUFFER_BYTES_MAX: usize = BUFFERED_UNFILTERED_LIMITS * ia64::UNFILTERED_BYTES_MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Encode,
    Decode,
}

const fn wrapping_position(now_pos: u32, offset: usize) -> u32 {
    now_pos.wrapping_add(offset as u32)
}

const fn translate(direction: Direction, value: u32, position: u32) -> u32 {
    match direction {
        Direction::Encode => value.wrapping_add(position),
        Direction::Decode => value.wrapping_sub(position),
    }
}

enum Filter {
    X86(x86::State),
    PowerPc,
    Ia64,
    Arm,
    ArmThumb,
    Sparc,
    Arm64,
    RiscV,
}

impl Filter {
    const fn new(architecture: BcjArchitecture) -> Self {
        match architecture {
            BcjArchitecture::X86 => Self::X86(x86::State::new()),
            BcjArchitecture::PowerPc => Self::PowerPc,
            BcjArchitecture::Ia64 => Self::Ia64,
            BcjArchitecture::Arm => Self::Arm,
            BcjArchitecture::ArmThumb => Self::ArmThumb,
            BcjArchitecture::Sparc => Self::Sparc,
            BcjArchitecture::Arm64 => Self::Arm64,
            BcjArchitecture::RiscV => Self::RiscV,
        }
    }

    const fn unfiltered_bytes_max(&self) -> usize {
        match self {
            Self::X86(_) => x86::UNFILTERED_BYTES_MAX,
            Self::PowerPc => powerpc::UNFILTERED_BYTES_MAX,
            Self::Ia64 => ia64::UNFILTERED_BYTES_MAX,
            Self::Arm => arm::UNFILTERED_BYTES_MAX,
            Self::ArmThumb => armthumb::UNFILTERED_BYTES_MAX,
            Self::Sparc => sparc::UNFILTERED_BYTES_MAX,
            Self::Arm64 => arm64::UNFILTERED_BYTES_MAX,
            Self::RiscV => riscv::UNFILTERED_BYTES_MAX,
        }
    }

    const fn alignment_bytes(&self) -> u32 {
        match self {
            Self::X86(_) => x86::ALIGNMENT_BYTES,
            Self::PowerPc => powerpc::ALIGNMENT_BYTES,
            Self::Ia64 => ia64::ALIGNMENT_BYTES,
            Self::Arm => arm::ALIGNMENT_BYTES,
            Self::ArmThumb => armthumb::ALIGNMENT_BYTES,
            Self::Sparc => sparc::ALIGNMENT_BYTES,
            Self::Arm64 => arm64::ALIGNMENT_BYTES,
            Self::RiscV => riscv::ALIGNMENT_BYTES,
        }
    }

    fn code(&mut self, now_pos: u32, direction: Direction, buffer: &mut [u8]) -> usize {
        match self {
            Self::X86(state) => x86::code(state, now_pos, direction, buffer),
            Self::PowerPc => powerpc::code(now_pos, direction, buffer),
            Self::Ia64 => ia64::code(now_pos, direction, buffer),
            Self::Arm => arm::code(now_pos, direction, buffer),
            Self::ArmThumb => armthumb::code(now_pos, direction, buffer),
            Self::Sparc => sparc::code(now_pos, direction, buffer),
            Self::Arm64 => arm64::code(now_pos, direction, buffer),
            Self::RiscV => match direction {
                Direction::Encode => riscv::encode(now_pos, buffer),
                Direction::Decode => riscv::decode(now_pos, buffer),
            },
        }
    }
}

enum Upstream {
    CallerInput,
    Coder(Coder),
}

struct SimpleCoder {
    upstream: Upstream,
    end_was_reached: bool,
    direction: Direction,
    filter: Filter,
    now_pos: u32,
    allocated: usize,
    position: usize,
    filtered: usize,
    size: usize,
    buffer: [u8; BUFFER_BYTES_MAX],
}

fn copy_or_code(
    upstream: &mut Upstream,
    end_was_reached: &mut bool,
    input: &[u8],
    input_position: &mut usize,
    output: &mut [u8],
    output_position: &mut usize,
    action: Action,
) -> Result<Status, Error> {
    match upstream {
        Upstream::CallerInput => {
            buffer_copy(input, input_position, output, output_position);
            if action == Action::Finish && *input_position == input.len() {
                *end_was_reached = true;
            }
        }
        Upstream::Coder(next) => {
            match next.code(input, input_position, output, output_position, action)? {
                Status::StreamEnd => *end_was_reached = true,
                Status::Ok => {}
                status => return Ok(status),
            }
        }
    }
    Ok(Status::Ok)
}

fn call_filter(
    filter: &mut Filter,
    now_pos: &mut u32,
    direction: Direction,
    buffer: &mut [u8],
) -> usize {
    if buffer.is_empty() {
        return 0;
    }
    let filtered = filter.code(*now_pos, direction, buffer);
    assert!(filtered <= buffer.len());
    assert!(buffer.len() - filtered < filter.unfiltered_bytes_max());
    *now_pos = wrapping_position(*now_pos, filtered);
    filtered
}

impl SimpleCoder {
    fn code_in_output(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        let output_start = *output_position;
        let buffer_available = self.size - self.position;
        output[output_start..output_start + buffer_available]
            .copy_from_slice(&self.buffer[self.position..self.size]);
        *output_position += buffer_available;
        let status = copy_or_code(
            &mut self.upstream,
            &mut self.end_was_reached,
            input,
            input_position,
            output,
            output_position,
            action,
        )?;
        if status != Status::Ok {
            return Ok(status);
        }
        assert!(*output_position >= output_start);
        let size = *output_position - output_start;
        let filtered = call_filter(
            &mut self.filter,
            &mut self.now_pos,
            self.direction,
            &mut output[output_start..*output_position],
        );
        let unfiltered = size - filtered;
        self.position = 0;
        self.size = unfiltered;
        if self.end_was_reached {
            self.size = 0;
        } else if unfiltered > 0 {
            *output_position -= unfiltered;
            self.buffer[..unfiltered]
                .copy_from_slice(&output[*output_position..*output_position + unfiltered]);
        }
        Ok(Status::Ok)
    }

    fn code_in_buffer(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        assert_eq!(self.position, 0);
        let status = copy_or_code(
            &mut self.upstream,
            &mut self.end_was_reached,
            input,
            input_position,
            &mut self.buffer[..self.allocated],
            &mut self.size,
            action,
        )?;
        if status != Status::Ok {
            return Ok(status);
        }
        self.filtered = call_filter(
            &mut self.filter,
            &mut self.now_pos,
            self.direction,
            &mut self.buffer[..self.size],
        );
        if self.end_was_reached {
            self.filtered = self.size;
        }
        buffer_copy(
            &self.buffer[..self.filtered],
            &mut self.position,
            output,
            output_position,
        );
        Ok(Status::Ok)
    }
}

impl Code for SimpleCoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        if self.position < self.filtered {
            buffer_copy(
                &self.buffer[..self.filtered],
                &mut self.position,
                output,
                output_position,
            );
            if self.position < self.filtered {
                return Ok(Status::Ok);
            }
            if self.end_was_reached {
                return Ok(Status::StreamEnd);
            }
        }
        self.filtered = 0;
        assert!(self.position <= self.size);
        assert!(*output_position <= output.len());
        let output_available = output.len() - *output_position;
        let buffer_available = self.size - self.position;
        if output_available > buffer_available || buffer_available == 0 {
            let status =
                self.code_in_output(input, input_position, output, output_position, action)?;
            if status != Status::Ok {
                return Ok(status);
            }
        } else if self.position > 0 {
            self.buffer.copy_within(self.position..self.size, 0);
            self.size -= self.position;
            self.position = 0;
        }
        if self.size > 0 {
            let status =
                self.code_in_buffer(input, input_position, output, output_position, action)?;
            if status != Status::Ok {
                return Ok(status);
            }
        }
        if self.end_was_reached && self.position == self.size {
            return Ok(Status::StreamEnd);
        }
        Ok(Status::Ok)
    }
}

pub(crate) const fn validate(
    architecture: BcjArchitecture,
    start_offset: u32,
) -> Result<(), Error> {
    let filter = Filter::new(architecture);
    if start_offset & (filter.alignment_bytes() - 1) != 0 {
        return Err(Error::Options);
    }
    Ok(())
}

fn new_coder(
    architecture: BcjArchitecture,
    start_offset: u32,
    direction: Direction,
    upstream: Upstream,
) -> Result<Coder, Error> {
    validate(architecture, start_offset)?;
    let filter = Filter::new(architecture);
    let allocated = BUFFERED_UNFILTERED_LIMITS * filter.unfiltered_bytes_max();
    assert!(allocated <= BUFFER_BYTES_MAX);
    Ok(Box::new(SimpleCoder {
        upstream,
        end_was_reached: false,
        direction,
        filter,
        now_pos: start_offset,
        allocated,
        position: 0,
        filtered: 0,
        size: 0,
        buffer: [0; BUFFER_BYTES_MAX],
    }))
}

pub(crate) fn decoder(
    architecture: BcjArchitecture,
    start_offset: u32,
    next: Coder,
) -> Result<Coder, Error> {
    new_coder(
        architecture,
        start_offset,
        Direction::Decode,
        Upstream::Coder(next),
    )
}

pub(crate) fn encoder(
    architecture: BcjArchitecture,
    start_offset: u32,
    next: Option<Coder>,
) -> Result<Coder, Error> {
    let upstream = match next {
        Some(next) => Upstream::Coder(next),
        None => Upstream::CallerInput,
    };
    new_coder(architecture, start_offset, Direction::Encode, upstream)
}
