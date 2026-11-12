use alloc::boxed::Box;

use super::alone::AloneDecoder;
use super::coder::{Code, Coder};
use super::stream::{Action, Error, Status};
use super::stream_decoder::StreamDecoder;
use super::stream_flags::HEADER_MAGIC;

enum Sequence {
    Init { memlimit: u64 },
    Code(Coder),
}

pub(crate) struct AutoDecoder {
    sequence: Sequence,
}

impl AutoDecoder {
    pub(crate) const fn new(memlimit: u64) -> Self {
        Self {
            sequence: Sequence::Init { memlimit },
        }
    }
}

impl Code for AutoDecoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        if let Sequence::Init { memlimit } = self.sequence {
            if *input_position >= input.len() {
                return Ok(Status::Ok);
            }
            if input[*input_position] == HEADER_MAGIC[0] {
                self.sequence = Sequence::Code(Box::new(StreamDecoder::new(memlimit)));
            } else {
                self.sequence = Sequence::Code(Box::new(AloneDecoder::new(memlimit, true)));
                return Ok(Status::NoCheck);
            }
        }
        let Sequence::Code(coder) = &mut self.sequence else {
            unreachable!("the auto decoder picked its format")
        };
        coder.code(input, input_position, output, output_position, action)
    }
}
