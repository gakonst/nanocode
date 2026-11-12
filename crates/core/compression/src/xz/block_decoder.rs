use super::block_header::{BlockHeader, unpadded_size};
use super::check::{CHECK_BYTES_MAX, CHECK_ID_NONE, CheckState, check_bytes};
use super::coder::{Code, Coder, buffer_copy};
use super::filter_chain;
use super::stream::{Action, Error, Status};
use super::vli::VLI_MAX;

const BLOCK_ALIGNMENT_MASK: u64 = 3;

enum Phase {
    Code(CheckState),
    Padding(CheckState),
    Check {
        computed: [u8; CHECK_BYTES_MAX],
        stored: [u8; CHECK_BYTES_MAX],
        position: usize,
    },
}

pub(crate) struct BlockDecoder {
    coder: Coder,
    phase: Phase,
    check_id: u32,
    header_size: usize,
    expected_compressed_size: Option<u64>,
    expected_uncompressed_size: Option<u64>,
    compressed_size: u64,
    uncompressed_size: u64,
    compressed_limit: u64,
    uncompressed_limit: u64,
    unpadded_compressed_size: u64,
}

impl BlockDecoder {
    pub(crate) fn new(header: &BlockHeader, check_id: u32) -> Result<Self, Error> {
        let container_bytes = (header.header_size + check_bytes(check_id)) as u64;
        let compressed_limit = header
            .compressed_size
            .unwrap_or((VLI_MAX & !BLOCK_ALIGNMENT_MASK) - container_bytes);
        let coder = filter_chain::raw_decoder(&header.filters)?;
        Ok(Self {
            coder,
            phase: Phase::Code(CheckState::new(check_id)),
            check_id,
            header_size: header.header_size,
            expected_compressed_size: header.compressed_size,
            expected_uncompressed_size: header.uncompressed_size,
            compressed_size: 0,
            uncompressed_size: 0,
            compressed_limit,
            uncompressed_limit: header.uncompressed_size.unwrap_or(VLI_MAX),
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
        let input_room = (self.compressed_limit - self.compressed_size)
            .min((input.len() - input_start) as u64) as usize;
        let output_room = (self.uncompressed_limit - self.uncompressed_size)
            .min((output.len() - output_start) as u64) as usize;
        let result = self.coder.code(
            &input[..input_start + input_room],
            input_position,
            &mut output[..output_start + output_room],
            output_position,
            action,
        );
        self.compressed_size += (*input_position - input_start) as u64;
        self.uncompressed_size += (*output_position - output_start) as u64;
        if let Ok(Status::Ok) = result {
            let compressed_done = Some(self.compressed_size) == self.expected_compressed_size;
            let uncompressed_done = Some(self.uncompressed_size) == self.expected_uncompressed_size;
            if compressed_done && uncompressed_done {
                return Err(Error::Data);
            }
            if compressed_done && *output_position < output.len() {
                return Err(Error::Data);
            }
            if uncompressed_done && *input_position < input.len() {
                return Err(Error::Data);
            }
        }
        if let Phase::Code(check) = &mut self.phase {
            check.update(&output[output_start..*output_position]);
        }
        result
    }

    fn finish_payload(&mut self) -> Result<(), Error> {
        if self
            .expected_compressed_size
            .is_some_and(|size| size != self.compressed_size)
            || self
                .expected_uncompressed_size
                .is_some_and(|size| size != self.uncompressed_size)
        {
            return Err(Error::Data);
        }
        self.unpadded_compressed_size = self.compressed_size;
        let phase = core::mem::replace(&mut self.phase, Phase::Code(CheckState::None));
        let Phase::Code(check) = phase else {
            unreachable!("the payload finishes in the code phase")
        };
        self.phase = Phase::Padding(check);
        Ok(())
    }

    fn skip_padding(&mut self, input: &[u8], input_position: &mut usize) -> Result<Status, Error> {
        while self.compressed_size & BLOCK_ALIGNMENT_MASK != 0 {
            if *input_position >= input.len() {
                return Ok(Status::Ok);
            }
            self.compressed_size += 1;
            let byte = input[*input_position];
            *input_position += 1;
            if byte != 0 {
                return Err(Error::Data);
            }
        }
        if self.check_id == CHECK_ID_NONE {
            return Ok(Status::StreamEnd);
        }
        let phase = core::mem::replace(&mut self.phase, Phase::Code(CheckState::None));
        let Phase::Padding(check) = phase else {
            unreachable!("padding is skipped in the padding phase")
        };
        self.phase = Phase::Check {
            computed: check.finish(),
            stored: [0; CHECK_BYTES_MAX],
            position: 0,
        };
        Ok(Status::Ok)
    }

    fn read_check(&mut self, input: &[u8], input_position: &mut usize) -> Result<Status, Error> {
        let Phase::Check {
            computed,
            stored,
            position,
        } = &mut self.phase
        else {
            unreachable!("the check is read in the check phase")
        };
        let size = check_bytes(self.check_id);
        buffer_copy(input, input_position, &mut stored[..size], position);
        if *position < size {
            return Ok(Status::Ok);
        }
        if super::stream::check_is_supported(self.check_id) && computed[..size] != stored[..size] {
            return Err(Error::Data);
        }
        Ok(Status::StreamEnd)
    }
}

impl Code for BlockDecoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        if matches!(self.phase, Phase::Code(_)) {
            let status =
                self.code_payload(input, input_position, output, output_position, action)?;
            if status != Status::StreamEnd {
                return Ok(status);
            }
            self.finish_payload()?;
        }
        if matches!(self.phase, Phase::Padding(_)) {
            let status = self.skip_padding(input, input_position)?;
            if !matches!(self.phase, Phase::Check { .. }) {
                return Ok(status);
            }
        }
        self.read_check(input, input_position)
    }
}
