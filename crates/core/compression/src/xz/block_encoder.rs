use super::block_header::{BLOCK_HEADER_BYTES_MAX, unpadded_size};
use super::check::{CHECK_BYTES_MAX, CHECK_ID_NONE, CheckState, check_bytes};
use super::coder::{Code, Coder, buffer_copy};
use super::filter_chain;
use super::options::Filter;
use super::stream::{Action, Error, Status};
use super::vli::VLI_MAX;

const BLOCK_ALIGNMENT_MASK: u64 = 3;
const COMPRESSED_SIZE_MAX: u64 =
    (VLI_MAX - BLOCK_HEADER_BYTES_MAX as u64 - CHECK_BYTES_MAX as u64) & !BLOCK_ALIGNMENT_MASK;

enum Phase {
    Code(CheckState),
    Padding(CheckState),
    Check {
        computed: [u8; CHECK_BYTES_MAX],
        position: usize,
    },
}

pub(crate) struct BlockEncoder {
    coder: Coder,
    phase: Phase,
    check_id: u32,
    pub(crate) header_size: usize,
    compressed_size: u64,
    uncompressed_size: u64,
    unpadded_compressed_size: u64,
}

impl BlockEncoder {
    pub(crate) fn new(
        filters: &[Filter],
        header_size: usize,
        check_id: u32,
    ) -> Result<Self, Error> {
        let coder = filter_chain::raw_encoder(filters)?;
        Ok(Self {
            coder,
            phase: Phase::Code(CheckState::new(check_id)),
            check_id,
            header_size,
            compressed_size: 0,
            uncompressed_size: 0,
            unpadded_compressed_size: 0,
        })
    }

    pub(crate) fn unpadded_size(&self) -> u64 {
        unpadded_size(
            self.header_size,
            self.unpadded_compressed_size,
            self.check_id,
        )
        .unwrap_or(0)
    }

    pub(crate) const fn uncompressed_size(&self) -> u64 {
        self.uncompressed_size
    }

    fn code_payload(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        let input_start = *input_position;
        let output_start = *output_position;
        let result = self
            .coder
            .code(input, input_position, output, output_position, action);
        let output_used = (*output_position - output_start) as u64;
        if COMPRESSED_SIZE_MAX - self.compressed_size < output_used {
            return Err(Error::Data);
        }
        self.compressed_size += output_used;
        self.uncompressed_size += (*input_position - input_start) as u64;
        if let Phase::Code(check) = &mut self.phase {
            check.update(&input[input_start..*input_position]);
        }
        result
    }

    fn write_padding(&mut self, output: &mut [u8], output_position: &mut usize) -> Status {
        while self.compressed_size & BLOCK_ALIGNMENT_MASK != 0 {
            if *output_position >= output.len() {
                return Status::Ok;
            }
            output[*output_position] = 0;
            *output_position += 1;
            self.compressed_size += 1;
        }
        if self.check_id == CHECK_ID_NONE {
            return Status::StreamEnd;
        }
        let phase = core::mem::replace(&mut self.phase, Phase::Code(CheckState::None));
        let Phase::Padding(check) = phase else {
            unreachable!("padding is written in the padding phase")
        };
        self.phase = Phase::Check {
            computed: check.finish(),
            position: 0,
        };
        Status::Ok
    }
}

impl Code for BlockEncoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        if VLI_MAX - self.uncompressed_size < (input.len() - *input_position) as u64 {
            return Err(Error::Data);
        }
        if matches!(self.phase, Phase::Code(_)) {
            let status =
                self.code_payload(input, input_position, output, output_position, action)?;
            if status != Status::StreamEnd {
                return Ok(status);
            }
            self.unpadded_compressed_size = self.compressed_size;
            let phase = core::mem::replace(&mut self.phase, Phase::Code(CheckState::None));
            let Phase::Code(check) = phase else {
                unreachable!("the payload finishes in the code phase")
            };
            self.phase = Phase::Padding(check);
        }
        if matches!(self.phase, Phase::Padding(_)) {
            let status = self.write_padding(output, output_position);
            if !matches!(self.phase, Phase::Check { .. }) {
                return Ok(status);
            }
        }
        let Phase::Check { computed, position } = &mut self.phase else {
            unreachable!("the check is written in the check phase")
        };
        let size = check_bytes(self.check_id);
        buffer_copy(&computed[..size], position, output, output_position);
        if *position < size {
            return Ok(Status::Ok);
        }
        Ok(Status::StreamEnd)
    }
}
