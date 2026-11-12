pub(crate) struct ByteReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ByteReader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub(crate) const fn skip(&mut self, count: usize) {
        self.position = self.position.wrapping_add(count);
    }

    pub(crate) const fn rewind(&mut self, count: usize) {
        self.position = self.position.saturating_sub(count);
    }

    pub(crate) const fn has(&self, count: usize) -> bool {
        self.position.saturating_add(count) <= self.bytes.len()
    }

    pub(crate) const fn eof(&self) -> bool {
        self.position >= self.bytes.len()
    }

    pub(crate) fn peek(&self, count: usize) -> Option<&'a [u8]> {
        let end = self.position.checked_add(count)?;
        self.bytes.get(self.position..end)
    }

    pub(crate) fn read_u8(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.position)?;
        self.position += 1;
        Some(byte)
    }

    pub(crate) fn read_u8_or_zero(&mut self) -> u8 {
        match self.bytes.get(self.position) {
            Some(&byte) => {
                self.position += 1;
                byte
            }
            None => 0,
        }
    }

    pub(crate) fn read_u16(&mut self) -> Option<u16> {
        let bytes = self.peek(2)?;
        let value = u16::from_be_bytes([bytes[0], bytes[1]]);
        self.position += 2;
        Some(value)
    }

    pub(crate) fn read_u16_or_zero(&mut self) -> u16 {
        self.read_u16().unwrap_or(0)
    }

    pub(crate) fn read_u32(&mut self) -> Option<u32> {
        let bytes = self.peek(4)?;
        let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        self.position += 4;
        Some(value)
    }

    pub(crate) fn read_exact(&mut self, buffer: &mut [u8]) -> Option<()> {
        assert!(self.position <= self.bytes.len());
        let available = &self.bytes[self.position..];
        let count = buffer.len().min(available.len());
        buffer[..count].copy_from_slice(&available[..count]);
        self.position += count;
        (count == buffer.len()).then_some(())
    }
}
