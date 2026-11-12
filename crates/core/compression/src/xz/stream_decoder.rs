use super::block_decoder::BlockDecoder;
use super::block_header::{self, BLOCK_HEADER_BYTES_MAX};
use super::check::CHECK_ID_NONE;
use super::coder::{Code, buffer_copy};
use super::filter_chain;
use super::index::{self, IndexHash};
use super::stream::{Action, Error, Status};
use super::stream_flags::{self, STREAM_HEADER_BYTES};

enum Sequence {
    StreamHeader,
    BlockHeader { header_size: usize },
    BlockInit { header_size: usize },
    BlockRun(BlockDecoder),
    Index,
    StreamFooter,
}

pub(crate) struct StreamDecoder {
    sequence: Sequence,
    index_hash: IndexHash,
    memlimit: u64,
    check_id: u32,
    position: usize,
    buffer: [u8; BLOCK_HEADER_BYTES_MAX],
}

impl StreamDecoder {
    pub(crate) fn new(memlimit: u64) -> Self {
        Self {
            sequence: Sequence::StreamHeader,
            index_hash: IndexHash::new(),
            memlimit: memlimit.max(1),
            check_id: CHECK_ID_NONE,
            position: 0,
            buffer: [0; BLOCK_HEADER_BYTES_MAX],
        }
    }

    fn stream_header(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
    ) -> Result<Option<Status>, Error> {
        buffer_copy(
            input,
            input_position,
            &mut self.buffer[..STREAM_HEADER_BYTES],
            &mut self.position,
        );
        if self.position < STREAM_HEADER_BYTES {
            return Ok(Some(Status::Ok));
        }
        self.position = 0;
        let mut header = [0; STREAM_HEADER_BYTES];
        header.copy_from_slice(&self.buffer[..STREAM_HEADER_BYTES]);
        self.check_id = stream_flags::decode_header(&header)?;
        self.sequence = Sequence::BlockHeader { header_size: 0 };
        if self.check_id == CHECK_ID_NONE {
            return Ok(Some(Status::NoCheck));
        }
        Ok(Some(Status::GetCheck))
    }

    fn block_header(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        header_size: usize,
    ) -> Option<Status> {
        if *input_position >= input.len() {
            return Some(Status::Ok);
        }
        let mut header_size = header_size;
        if self.position == 0 {
            let byte = input[*input_position];
            if index::is_indicator(byte) {
                self.sequence = Sequence::Index;
                return None;
            }
            header_size = block_header::header_size_from_byte(byte);
            self.sequence = Sequence::BlockHeader { header_size };
        }
        buffer_copy(
            input,
            input_position,
            &mut self.buffer[..header_size],
            &mut self.position,
        );
        if self.position < header_size {
            return Some(Status::Ok);
        }
        self.position = 0;
        self.sequence = Sequence::BlockInit { header_size };
        None
    }

    fn block_init(&mut self, header_size: usize) -> Result<(), Error> {
        let header = block_header::decode(&self.buffer[..header_size], self.check_id)?;
        let memusage = filter_chain::raw_decoder_memusage(&header.filters).ok_or(Error::Options)?;
        if memusage > self.memlimit {
            return Err(Error::MemoryLimit);
        }
        let block = BlockDecoder::new(&header, self.check_id)?;
        self.sequence = Sequence::BlockRun(block);
        Ok(())
    }

    fn stream_footer(&mut self, input: &[u8], input_position: &mut usize) -> Result<Status, Error> {
        buffer_copy(
            input,
            input_position,
            &mut self.buffer[..STREAM_HEADER_BYTES],
            &mut self.position,
        );
        if self.position < STREAM_HEADER_BYTES {
            return Ok(Status::Ok);
        }
        self.position = 0;
        let mut footer = [0; STREAM_HEADER_BYTES];
        footer.copy_from_slice(&self.buffer[..STREAM_HEADER_BYTES]);
        let flags = stream_flags::decode_footer(&footer).map_err(|error| match error {
            Error::Format => Error::Data,
            other => other,
        })?;
        if self.index_hash.size() != flags.backward_size {
            return Err(Error::Data);
        }
        if flags.check_id != self.check_id {
            return Err(Error::Data);
        }
        Ok(Status::StreamEnd)
    }
}

impl Code for StreamDecoder {
    fn code(
        &mut self,
        input: &[u8],
        input_position: &mut usize,
        output: &mut [u8],
        output_position: &mut usize,
        action: Action,
    ) -> Result<Status, Error> {
        loop {
            match &mut self.sequence {
                Sequence::StreamHeader => {
                    if let Some(status) = self.stream_header(input, input_position)? {
                        return Ok(status);
                    }
                }
                Sequence::BlockHeader { header_size } => {
                    let header_size = *header_size;
                    if let Some(status) = self.block_header(input, input_position, header_size) {
                        return Ok(status);
                    }
                }
                Sequence::BlockInit { header_size } => {
                    let header_size = *header_size;
                    self.block_init(header_size)?;
                }
                Sequence::BlockRun(block) => {
                    let status =
                        block.code(input, input_position, output, output_position, action)?;
                    if status != Status::StreamEnd {
                        return Ok(status);
                    }
                    let unpadded_size = block.unpadded_size();
                    let uncompressed_size = block.uncompressed_size();
                    self.index_hash.append(unpadded_size, uncompressed_size)?;
                    self.sequence = Sequence::BlockHeader { header_size: 0 };
                }
                Sequence::Index => {
                    if *input_position >= input.len() {
                        return Ok(Status::Ok);
                    }
                    let status = self.index_hash.decode(input, input_position)?;
                    if status != Status::StreamEnd {
                        return Ok(status);
                    }
                    self.sequence = Sequence::StreamFooter;
                }
                Sequence::StreamFooter => return self.stream_footer(input, input_position),
            }
        }
    }
}
