use crate::error::{Error, Result};

pub(crate) struct DecodeOut<'a> {
    buf: &'a mut Vec<u8>,
}

impl<'a> DecodeOut<'a> {
    pub(crate) const fn growable(buf: &'a mut Vec<u8>) -> Self {
        Self { buf }
    }

    pub(crate) const fn len(&self) -> usize {
        self.buf.len()
    }

    pub(crate) const fn buffer_mut(&mut self) -> &mut Vec<u8> {
        self.buf
    }

    pub(crate) fn try_reserve(&mut self, additional: usize) -> Result<()> {
        self.buf
            .try_reserve(additional)
            .map_err(|_| Error::OutputSizeOverflow)
    }

    pub(crate) fn ensure_room(&mut self, additional: usize) -> Result<()> {
        self.try_reserve(additional)
    }

    pub(crate) fn append(&mut self, bytes: &[u8]) -> Result<()> {
        self.ensure_room(bytes.len())?;
        self.buf.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn append_repeated(&mut self, byte: u8, count: usize) -> Result<()> {
        self.ensure_room(count)?;
        self.buf.resize(self.buf.len() + count, byte);
        Ok(())
    }
}
