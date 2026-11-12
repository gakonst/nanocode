use alloc::vec::Vec;

use super::block_encoder::BlockEncoder;
use super::block_header::{self, BLOCK_HEADER_BYTES_MAX};
use super::coder::{Code, buffer_copy};
use super::filter_chain::FILTERS_MAX;
use super::index::IndexEncoder;
use super::options::{Check, Filter};
use super::stream::{Action, Error, Status};
use super::stream_flags::{self, STREAM_HEADER_BYTES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sequence {
    StreamHeader,
    BlockInit,
    BlockHeader,
    BlockEncode,
    IndexEncode,
    StreamFooter,
}

pub(crate) struct StreamEncoder {
    sequence: Sequence,
    filters: Vec<Filter>,
    check_id: u32,
    block: Option<BlockEncoder>,
    index: IndexEncoder,
    buffer: [u8; BLOCK_HEADER_BYTES_MAX],
    buffer_position: usize,
    buffer_size: usize,
}

fn block_encoder(filters: &[Filter], check_id: u32) -> Result<BlockEncoder, Error> {
    let header_size = block_header::encoded_size(filters)?;
    BlockEncoder::new(filters, header_size, check_id)
}

impl StreamEncoder {
    pub(crate) fn new(filters: &[Filter], check: Check) -> Result<Self, Error> {
        if filters.len() > FILTERS_MAX {
            return Err(Error::Options);
        }
        let check_id = check.id();
        let mut buffer = [0; BLOCK_HEADER_BYTES_MAX];
        buffer[..STREAM_HEADER_BYTES].copy_from_slice(&stream_flags::encode_header(check_id));
        let block = block_encoder(filters, check_id)?;
        let mut copy = Vec::new();
        copy.try_reserve_exact(filters.len())
            .map_err(|_| Error::Memory)?;
        copy.extend_from_slice(filters);
        Ok(Self {
            sequence: Sequence::StreamHeader,
            filters: copy,
            check_id,
            block: Some(block),
            index: IndexEncoder::new(),
            buffer,
            buffer_position: 0,
            buffer_size: STREAM_HEADER_BYTES,
        })
    }

    fn block_init(
        &mut self,
        input: &[u8],
        input_position: usize,
        action: Action,
    ) -> Result<Option<Status>, Error> {
        if input_position == input.len() {
            if action != Action::Finish {
                return Ok(Some(Status::Ok));
            }
            self.sequence = Sequence::IndexEncode;
            return Ok(None);
        }
        if self.block.is_none() {
            self.block = Some(block_encoder(&self.filters, self.check_id)?);
        }
        let block = self
            .block
            .as_ref()
            .expect("the block encoder was just initialized");
        block_header::encode(&self.filters, block.header_size, &mut self.buffer);
        self.buffer_size = block.header_size;
        self.sequence = Sequence::BlockHeader;
        Ok(None)
    }
}

impl Code for StreamEncoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        while *output_position < output.len() {
            match self.sequence {
                Sequence::StreamHeader | Sequence::BlockHeader | Sequence::StreamFooter => {
                    buffer_copy(
                        &self.buffer[..self.buffer_size],
                        &mut self.buffer_position,
                        output,
                        output_position,
                    );
                    if self.buffer_position < self.buffer_size {
                        return Ok(Status::Ok);
                    }
                    self.buffer_position = 0;
                    self.sequence = match self.sequence {
                        Sequence::StreamHeader => Sequence::BlockInit,
                        Sequence::BlockHeader => Sequence::BlockEncode,
                        _ => return Ok(Status::StreamEnd),
                    };
                }
                Sequence::BlockInit => {
                    if let Some(status) = self.block_init(input, *input_position, action)? {
                        return Ok(status);
                    }
                }
                Sequence::BlockEncode => {
                    let block = self.block.as_mut().expect("a block is being encoded");
                    let status =
                        block.code(input, input_position, output, output_position, action)?;
                    if status != Status::StreamEnd {
                        return Ok(status);
                    }
                    self.index
                        .append(block.unpadded_size(), block.uncompressed_size())?;
                    self.block = None;
                    self.sequence = Sequence::BlockInit;
                }
                Sequence::IndexEncode => {
                    let status = self.index.encode(output, output_position);
                    if status != Status::StreamEnd {
                        return Ok(status);
                    }
                    let footer = stream_flags::encode_footer(self.check_id, self.index.size());
                    self.buffer[..STREAM_HEADER_BYTES].copy_from_slice(&footer);
                    self.buffer_size = STREAM_HEADER_BYTES;
                    self.sequence = Sequence::StreamFooter;
                }
            }
        }
        Ok(Status::Ok)
    }
}
