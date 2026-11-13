use crate::error::WebpError;

pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    position: u64,
}

impl<'a> Cursor<'a> {
    pub(crate) const fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub(crate) const fn position(&self) -> u64 {
        self.position
    }

    pub(crate) const fn seek(&mut self, position: u64) {
        self.position = position;
    }

    pub(crate) fn seek_relative(&mut self, offset: i64) -> Result<(), WebpError> {
        self.position = self
            .position
            .checked_add_signed(offset)
            .ok_or(WebpError::SeekBeforeStart)?;
        Ok(())
    }

    pub(crate) fn read(&mut self, count: usize) -> Result<&'a [u8], WebpError> {
        if count == 0 {
            return Ok(&[]);
        }
        let remaining = self.remaining();
        if remaining.len() < count {
            return Err(WebpError::Truncated);
        }
        self.position += count as u64;
        Ok(&remaining[..count])
    }

    pub(crate) fn read_array<const COUNT: usize>(&mut self) -> Result<[u8; COUNT], WebpError> {
        let bytes = self.read(COUNT)?;
        let mut array = [0; COUNT];
        array.copy_from_slice(bytes);
        Ok(array)
    }

    pub(crate) fn read_u8(&mut self) -> Result<u8, WebpError> {
        Ok(self.read_array::<1>()?[0])
    }

    pub(crate) fn read_u24(&mut self) -> Result<u32, WebpError> {
        let [low, middle, high] = self.read_array::<3>()?;
        Ok(u32::from(low) | (u32::from(middle) << 8) | (u32::from(high) << 16))
    }

    pub(crate) fn read_u16(&mut self) -> Result<u16, WebpError> {
        Ok(u16::from_le_bytes(self.read_array::<2>()?))
    }

    pub(crate) fn read_u32(&mut self) -> Result<u32, WebpError> {
        Ok(u32::from_le_bytes(self.read_array::<4>()?))
    }

    pub(crate) fn remaining(&self) -> &'a [u8] {
        match usize::try_from(self.position) {
            Ok(position) if position < self.data.len() => &self.data[position..],
            _ => &[],
        }
    }

    pub(crate) fn take(&self, count: u64) -> &'a [u8] {
        let remaining = self.remaining();
        match usize::try_from(count) {
            Ok(count) if count < remaining.len() => &remaining[..count],
            _ => remaining,
        }
    }

    pub(crate) fn range(&mut self, start: u64, end: u64) -> &'a [u8] {
        assert!(start <= end);
        self.seek(start);
        self.take(end - start)
    }
}
