use std::io;

pub(crate) const BUFFER_CAPACITY_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum BinaryDetectionMode {
    #[default]
    None,
    Quit(u8),
    Convert(u8),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LineBuffer {
    line_terminator: u8,
    binary: BinaryDetectionMode,
    bytes: Vec<u8>,
    position: usize,
    last_line_terminator: usize,
    end: usize,
    absolute_byte_offset: u64,
    binary_byte_offset: Option<u64>,
}

impl LineBuffer {
    pub(crate) fn new() -> LineBuffer {
        LineBuffer {
            bytes: vec![0; BUFFER_CAPACITY_BYTES],
            ..LineBuffer::default()
        }
    }

    pub(crate) fn begin(&mut self, line_terminator: u8, binary: BinaryDetectionMode) {
        self.line_terminator = line_terminator;
        self.binary = binary;
        self.position = 0;
        self.last_line_terminator = 0;
        self.end = 0;
        self.absolute_byte_offset = 0;
        self.binary_byte_offset = None;
    }

    pub(crate) fn absolute_byte_offset(&self) -> u64 {
        self.absolute_byte_offset
    }

    pub(crate) fn binary_byte_offset(&self) -> Option<u64> {
        self.binary_byte_offset
    }

    pub(crate) fn buffer(&self) -> &[u8] {
        &self.bytes[self.position..self.last_line_terminator]
    }

    pub(crate) fn consume(&mut self, amount: usize) {
        assert!(
            amount <= self.buffer().len(),
            "consumed {amount} bytes past the buffer"
        );
        self.position += amount;
        self.absolute_byte_offset += amount as u64;
    }

    pub(crate) fn fill<R: io::Read + ?Sized>(&mut self, reader: &mut R) -> io::Result<bool> {
        if matches!(self.binary, BinaryDetectionMode::Quit(_)) && self.binary_byte_offset.is_some()
        {
            return Ok(!self.buffer().is_empty());
        }
        self.roll();
        assert_eq!(self.position, 0);
        loop {
            self.ensure_capacity();
            let read = reader.read(&mut self.bytes[self.end..])?;
            if read == 0 {
                self.last_line_terminator = self.end;
                return Ok(!self.buffer().is_empty());
            }
            let old_end = self.end;
            self.end += read;
            match self.binary {
                BinaryDetectionMode::None => {}
                BinaryDetectionMode::Quit(byte) => {
                    if let Some(found) = memchr::memchr(byte, &self.bytes[old_end..self.end]) {
                        self.end = old_end + found;
                        self.last_line_terminator = self.end;
                        self.binary_byte_offset = Some(self.absolute_byte_offset + self.end as u64);
                        return Ok(self.position < self.end);
                    }
                }
                BinaryDetectionMode::Convert(byte) => {
                    let new_bytes = &mut self.bytes[old_end..self.end];
                    if let Some(found) = replace_bytes(new_bytes, byte, self.line_terminator)
                        && self.binary_byte_offset.is_none()
                    {
                        self.binary_byte_offset =
                            Some(self.absolute_byte_offset + (old_end + found) as u64);
                    }
                }
            }
            if let Some(found) =
                memchr::memrchr(self.line_terminator, &self.bytes[old_end..self.end])
            {
                self.last_line_terminator = old_end + found + 1;
                return Ok(true);
            }
        }
    }

    fn roll(&mut self) {
        let leftover = self.end - self.position;
        self.bytes.copy_within(self.position..self.end, 0);
        self.position = 0;
        self.last_line_terminator = leftover;
        self.end = leftover;
    }

    fn ensure_capacity(&mut self) {
        if self.end < self.bytes.len() {
            return;
        }
        let grown = self.bytes.len() + self.bytes.len().max(1) * 2;
        self.bytes.resize(grown, 0);
    }
}

fn replace_bytes(bytes: &mut [u8], source: u8, replacement: u8) -> Option<usize> {
    if source == replacement {
        return None;
    }
    let first = memchr::memchr(source, bytes)?;
    for byte in &mut bytes[first..] {
        if *byte == source {
            *byte = replacement;
        }
    }
    Some(first)
}
