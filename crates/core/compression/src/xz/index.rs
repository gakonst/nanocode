use alloc::vec::Vec;

use super::block_header::{UNPADDED_SIZE_MAX, UNPADDED_SIZE_MIN};
use super::crc32;
use super::sha256::Sha256;
use super::stream::{Error, Status};
use super::stream_flags::{BACKWARD_SIZE_MAX, STREAM_HEADER_BYTES};
use super::vli::{self, Progress, VLI_MAX};

const INDEX_INDICATOR: u8 = 0;
const INDEX_INDICATOR_BYTES: u64 = 1;
const INDEX_UNIT_BYTES: u64 = 4;
const INDEX_CRC_BYTES: usize = 4;
const SHA256_BYTES: usize = 32;
const BYTE_BITS: usize = 8;

pub(crate) const fn is_indicator(byte: u8) -> bool {
    byte == INDEX_INDICATOR
}

const fn ceil4(value: u64) -> u64 {
    value.wrapping_add(INDEX_UNIT_BYTES - 1) & !(INDEX_UNIT_BYTES - 1)
}

fn size_unpadded(count: u64, index_list_size: u64) -> u64 {
    INDEX_INDICATOR_BYTES
        .wrapping_add(vli::size(count) as u64)
        .wrapping_add(index_list_size)
}

pub(crate) fn size(count: u64, index_list_size: u64) -> u64 {
    ceil4(size_unpadded(count, index_list_size)).wrapping_add(INDEX_CRC_BYTES as u64)
}

fn padding_bytes(count: u64, index_list_size: u64) -> usize {
    (INDEX_UNIT_BYTES.wrapping_sub(size_unpadded(count, index_list_size)) & (INDEX_UNIT_BYTES - 1))
        as usize
}

fn stream_size(blocks_size: u64, count: u64, index_list_size: u64) -> u64 {
    (STREAM_HEADER_BYTES as u64)
        .wrapping_add(blocks_size)
        .wrapping_add(size(count, index_list_size))
        .wrapping_add(STREAM_HEADER_BYTES as u64)
}

struct HashInfo {
    blocks_size: u64,
    uncompressed_size: u64,
    count: u64,
    index_list_size: u64,
    check: Sha256,
}

impl HashInfo {
    const fn new() -> Self {
        Self {
            blocks_size: 0,
            uncompressed_size: 0,
            count: 0,
            index_list_size: 0,
            check: Sha256::new(),
        }
    }

    fn append(&mut self, unpadded_size: u64, uncompressed_size: u64) {
        self.blocks_size = self.blocks_size.wrapping_add(ceil4(unpadded_size));
        self.uncompressed_size = self.uncompressed_size.wrapping_add(uncompressed_size);
        let list_bytes = vli::size(unpadded_size) + vli::size(uncompressed_size);
        self.index_list_size = self.index_list_size.wrapping_add(list_bytes as u64);
        self.count = self.count.wrapping_add(1);
        self.check.update(&unpadded_size.to_le_bytes());
        self.check.update(&uncompressed_size.to_le_bytes());
    }

    fn finish_check(&mut self) -> [u8; SHA256_BYTES] {
        core::mem::replace(&mut self.check, Sha256::new()).finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HashSequence {
    Block,
    Count,
    Unpadded,
    Uncompressed,
    PaddingInit,
    Padding,
    Crc32,
}

pub(crate) struct IndexHash {
    sequence: HashSequence,
    blocks: HashInfo,
    records: HashInfo,
    remaining: u64,
    unpadded_size: u64,
    uncompressed_size: u64,
    position: usize,
    crc32: u32,
}

impl IndexHash {
    pub(crate) const fn new() -> Self {
        Self {
            sequence: HashSequence::Block,
            blocks: HashInfo::new(),
            records: HashInfo::new(),
            remaining: 0,
            unpadded_size: 0,
            uncompressed_size: 0,
            position: 0,
            crc32: 0,
        }
    }

    pub(crate) fn size(&self) -> u64 {
        size(self.blocks.count, self.blocks.index_list_size)
    }

    pub(crate) fn append(
        &mut self,
        unpadded_size: u64,
        uncompressed_size: u64,
    ) -> Result<(), Error> {
        if self.sequence != HashSequence::Block
            || !(UNPADDED_SIZE_MIN..=UNPADDED_SIZE_MAX).contains(&unpadded_size)
            || uncompressed_size > VLI_MAX
        {
            return Err(Error::Program);
        }
        self.blocks.append(unpadded_size, uncompressed_size);
        let blocks = &self.blocks;
        if blocks.blocks_size > VLI_MAX
            || blocks.uncompressed_size > VLI_MAX
            || size(blocks.count, blocks.index_list_size) > BACKWARD_SIZE_MAX
            || stream_size(blocks.blocks_size, blocks.count, blocks.index_list_size) > VLI_MAX
        {
            return Err(Error::Data);
        }
        Ok(())
    }

    const fn read_crc32(&mut self, input: &[u8], position: &mut usize) -> Result<Status, Error> {
        while *position < input.len() {
            let byte = input[*position];
            *position += 1;
            if (self.crc32 >> (self.position * BYTE_BITS)) as u8 != byte {
                return Err(Error::Data);
            }
            self.position += 1;
            if self.position >= INDEX_CRC_BYTES {
                return Ok(Status::StreamEnd);
            }
        }
        Ok(Status::Ok)
    }

    fn record_size(&mut self, input: &[u8], position: &mut usize) -> Result<Progress, Error> {
        let unpadded = self.sequence == HashSequence::Unpadded;
        let value = if unpadded {
            &mut self.unpadded_size
        } else {
            &mut self.uncompressed_size
        };
        if vli::decode(value, &mut self.position, input, position)? == Progress::Incomplete {
            return Ok(Progress::Incomplete);
        }
        self.position = 0;
        if unpadded {
            if !(UNPADDED_SIZE_MIN..=UNPADDED_SIZE_MAX).contains(&self.unpadded_size) {
                return Err(Error::Data);
            }
            self.sequence = HashSequence::Uncompressed;
            return Ok(Progress::Complete);
        }
        self.records
            .append(self.unpadded_size, self.uncompressed_size);
        if self.blocks.blocks_size < self.records.blocks_size
            || self.blocks.uncompressed_size < self.records.uncompressed_size
            || self.blocks.index_list_size < self.records.index_list_size
        {
            return Err(Error::Data);
        }
        self.remaining -= 1;
        self.sequence = self.after_record();
        Ok(Progress::Complete)
    }

    const fn after_record(&self) -> HashSequence {
        if self.remaining == 0 {
            HashSequence::PaddingInit
        } else {
            HashSequence::Unpadded
        }
    }

    fn finish_records(
        &mut self,
        input: &[u8],
        start: usize,
        position: &mut usize,
    ) -> Result<Status, Error> {
        if self.blocks.blocks_size != self.records.blocks_size
            || self.blocks.uncompressed_size != self.records.uncompressed_size
            || self.blocks.index_list_size != self.records.index_list_size
        {
            return Err(Error::Data);
        }
        if self.blocks.finish_check() != self.records.finish_check() {
            return Err(Error::Data);
        }
        self.crc32 = crc32::update(self.crc32, &input[start..*position]);
        self.sequence = HashSequence::Crc32;
        self.read_crc32(input, position)
    }

    pub(crate) fn decode(&mut self, input: &[u8], position: &mut usize) -> Result<Status, Error> {
        assert!(*position < input.len());
        let start = *position;
        while *position < input.len() {
            match self.sequence {
                HashSequence::Block => {
                    let byte = input[*position];
                    *position += 1;
                    if byte != INDEX_INDICATOR {
                        return Err(Error::Data);
                    }
                    self.sequence = HashSequence::Count;
                }
                HashSequence::Count => {
                    let progress =
                        vli::decode(&mut self.remaining, &mut self.position, input, position)?;
                    if progress == Progress::Incomplete {
                        break;
                    }
                    if self.remaining != self.blocks.count {
                        return Err(Error::Data);
                    }
                    self.position = 0;
                    self.sequence = self.after_record();
                }
                HashSequence::Unpadded | HashSequence::Uncompressed => {
                    if self.record_size(input, position)? == Progress::Incomplete {
                        break;
                    }
                }
                HashSequence::PaddingInit => {
                    self.position = padding_bytes(self.records.count, self.records.index_list_size);
                    self.sequence = HashSequence::Padding;
                }
                HashSequence::Padding => {
                    if self.position == 0 {
                        return self.finish_records(input, start, position);
                    }
                    self.position -= 1;
                    let byte = input[*position];
                    *position += 1;
                    if byte != 0 {
                        return Err(Error::Data);
                    }
                }
                HashSequence::Crc32 => return self.read_crc32(input, position),
            }
        }
        self.crc32 = crc32::update(self.crc32, &input[start..*position]);
        Ok(Status::Ok)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EncoderSequence {
    Indicator,
    Count,
    Next,
    Unpadded,
    Uncompressed,
    Padding,
    Crc32,
}

pub(crate) struct IndexEncoder {
    records: Vec<(u64, u64)>,
    index_list_size: u64,
    sequence: EncoderSequence,
    next_record: usize,
    record: (u64, u64),
    position: usize,
    crc32: u32,
}

impl IndexEncoder {
    pub(crate) const fn new() -> Self {
        Self {
            records: Vec::new(),
            index_list_size: 0,
            sequence: EncoderSequence::Indicator,
            next_record: 0,
            record: (0, 0),
            position: 0,
            crc32: 0,
        }
    }

    pub(crate) fn append(
        &mut self,
        unpadded_size: u64,
        uncompressed_size: u64,
    ) -> Result<(), Error> {
        assert_eq!(self.sequence, EncoderSequence::Indicator);
        if !(UNPADDED_SIZE_MIN..=UNPADDED_SIZE_MAX).contains(&unpadded_size)
            || uncompressed_size > VLI_MAX
        {
            return Err(Error::Program);
        }
        let list_bytes = (vli::size(unpadded_size) + vli::size(uncompressed_size)) as u64;
        let count = self.records.len() as u64 + 1;
        if size(count, self.index_list_size + list_bytes) > BACKWARD_SIZE_MAX {
            return Err(Error::Data);
        }
        self.records.try_reserve(1).map_err(|_| Error::Memory)?;
        self.records.push((unpadded_size, uncompressed_size));
        self.index_list_size += list_bytes;
        Ok(())
    }

    pub(crate) fn size(&self) -> u64 {
        size(self.records.len() as u64, self.index_list_size)
    }

    const fn write_crc32(&mut self, output: &mut [u8], position: &mut usize) -> Status {
        while *position < output.len() {
            output[*position] = (self.crc32 >> (self.position * BYTE_BITS)) as u8;
            *position += 1;
            self.position += 1;
            if self.position >= INDEX_CRC_BYTES {
                return Status::StreamEnd;
            }
        }
        Status::Ok
    }

    fn write_record_size(&mut self, output: &mut [u8], position: &mut usize) -> Progress {
        let unpadded = self.sequence == EncoderSequence::Unpadded;
        let value = if unpadded {
            self.record.0
        } else {
            self.record.1
        };
        if vli::encode(value, &mut self.position, output, position) == Progress::Incomplete {
            return Progress::Incomplete;
        }
        self.position = 0;
        self.sequence = if unpadded {
            EncoderSequence::Uncompressed
        } else {
            EncoderSequence::Next
        };
        Progress::Complete
    }

    pub(crate) fn encode(&mut self, output: &mut [u8], position: &mut usize) -> Status {
        let start = *position;
        while *position < output.len() {
            match self.sequence {
                EncoderSequence::Indicator => {
                    output[*position] = INDEX_INDICATOR;
                    *position += 1;
                    self.sequence = EncoderSequence::Count;
                }
                EncoderSequence::Count => {
                    let count = self.records.len() as u64;
                    if vli::encode(count, &mut self.position, output, position)
                        == Progress::Incomplete
                    {
                        break;
                    }
                    self.position = 0;
                    self.sequence = EncoderSequence::Next;
                }
                EncoderSequence::Next => {
                    if self.next_record == self.records.len() {
                        self.position =
                            padding_bytes(self.records.len() as u64, self.index_list_size);
                        self.sequence = EncoderSequence::Padding;
                    } else {
                        self.record = self.records[self.next_record];
                        self.next_record += 1;
                        self.sequence = EncoderSequence::Unpadded;
                    }
                }
                EncoderSequence::Unpadded | EncoderSequence::Uncompressed => {
                    if self.write_record_size(output, position) == Progress::Incomplete {
                        break;
                    }
                }
                EncoderSequence::Padding => {
                    if self.position > 0 {
                        self.position -= 1;
                        output[*position] = 0;
                        *position += 1;
                    } else {
                        self.crc32 = crc32::update(self.crc32, &output[start..*position]);
                        self.sequence = EncoderSequence::Crc32;
                        return self.write_crc32(output, position);
                    }
                }
                EncoderSequence::Crc32 => return self.write_crc32(output, position),
            }
        }
        self.crc32 = crc32::update(self.crc32, &output[start..*position]);
        Status::Ok
    }
}
