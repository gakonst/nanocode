use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use super::alone::{AloneDecoder, AloneEncoder};
use super::auto_decoder::AutoDecoder;
use super::coder::Coder;
use super::filter_chain;
use super::options::{Check, Filter, LzmaOptions};
use super::stream_decoder::StreamDecoder;
use super::stream_encoder::StreamEncoder;

const CHECK_ID_MAX: u32 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Run,
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    StreamEnd,
    GetCheck,
    NoCheck,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Memory,
    MemoryLimit,
    Format,
    Options,
    Data,
    Program,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Memory => "cannot allocate memory",
            Self::MemoryLimit => "memory usage limit was reached",
            Self::Format => "file format not recognized",
            Self::Options => "invalid or unsupported options",
            Self::Data => "data is corrupt",
            Self::Program => "the stream was used in an invalid sequence",
        })
    }
}

#[must_use]
pub const fn check_is_supported(check_id: u32) -> bool {
    check_id <= CHECK_ID_MAX
        && (check_id == Check::None.id()
            || check_id == Check::Crc32.id()
            || check_id == Check::Crc64.id()
            || check_id == Check::Sha256.id())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sequence {
    Run,
    Finish { input_remaining: usize },
    End,
    Failed,
}

pub struct Stream {
    coder: Coder,
    sequence: Sequence,
    total_in: u64,
    total_out: u64,
}

impl Stream {
    pub(crate) fn new(coder: Coder) -> Self {
        Self {
            coder,
            sequence: Sequence::Run,
            total_in: 0,
            total_out: 0,
        }
    }

    pub fn easy_encoder(preset: u32, check: Check) -> Result<Self, Error> {
        let options = LzmaOptions::preset(preset)?;
        Self::stream_encoder(&[Filter::Lzma2(options)], check)
    }

    pub fn stream_encoder(filters: &[Filter], check: Check) -> Result<Self, Error> {
        let encoder = StreamEncoder::new(filters, check)?;
        Ok(Self::new(Box::new(encoder)))
    }

    pub fn alone_encoder(options: &LzmaOptions) -> Result<Self, Error> {
        let encoder = AloneEncoder::new(options)?;
        Ok(Self::new(Box::new(encoder)))
    }

    #[must_use]
    pub fn auto_decoder(memlimit: u64) -> Self {
        Self::new(Box::new(AutoDecoder::new(memlimit)))
    }

    #[must_use]
    pub fn stream_decoder(memlimit: u64) -> Self {
        Self::new(Box::new(StreamDecoder::new(memlimit)))
    }

    #[must_use]
    pub fn alone_decoder(memlimit: u64) -> Self {
        Self::new(Box::new(AloneDecoder::new(memlimit, false)))
    }

    pub fn raw_encoder(filters: &[Filter]) -> Result<Self, Error> {
        filter_chain::raw_encoder(filters).map(Self::new)
    }

    pub fn raw_decoder(filters: &[Filter]) -> Result<Self, Error> {
        filter_chain::raw_decoder(filters).map(Self::new)
    }

    pub fn process(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        action: Action,
    ) -> Result<Status, Error> {
        match (self.sequence, action) {
            (Sequence::Run, _) => {}
            (Sequence::Finish { input_remaining }, Action::Finish)
                if input_remaining == input.len() => {}
            (Sequence::Finish { .. }, _) | (Sequence::Failed, _) => return Err(Error::Program),
            (Sequence::End, _) => return Ok(Status::StreamEnd),
        }
        let mut input_position = 0;
        let mut output_position = 0;
        let result = self.coder.code(
            input,
            &mut input_position,
            output,
            &mut output_position,
            action,
        );
        assert!(input_position <= input.len());
        assert!(output_position <= output.len());
        self.total_in += u64::try_from(input_position).expect("a slice length fits in 64 bits");
        self.total_out += u64::try_from(output_position).expect("a slice length fits in 64 bits");
        self.sequence = match (&result, action) {
            (Ok(Status::StreamEnd), _) => Sequence::End,
            (Ok(_) | Err(Error::MemoryLimit), Action::Run) => Sequence::Run,
            (Ok(_) | Err(Error::MemoryLimit), Action::Finish) => Sequence::Finish {
                input_remaining: input.len() - input_position,
            },
            (Err(_), _) => Sequence::Failed,
        };
        result
    }

    pub fn process_vec(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
        action: Action,
    ) -> Result<Status, Error> {
        let length = output.len();
        output.resize(output.capacity(), 0);
        let total_out = self.total_out;
        let result = self.process(input, &mut output[length..], action);
        let produced = usize::try_from(self.total_out - total_out)
            .expect("bytes produced into a slice fit in usize");
        output.truncate(length + produced);
        result
    }

    #[must_use]
    pub const fn total_in(&self) -> u64 {
        self.total_in
    }

    #[must_use]
    pub const fn total_out(&self) -> u64 {
        self.total_out
    }
}
