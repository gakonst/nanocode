#[derive(Clone)]
pub(super) struct BlockBuffer<const BLOCK_BYTES: usize> {
    block: [u8; BLOCK_BYTES],
    filled: usize,
}

const PADDING_MARKER: u8 = 0x80;

impl<const BLOCK_BYTES: usize> BlockBuffer<BLOCK_BYTES> {
    pub(super) const fn new() -> Self {
        Self {
            block: [0; BLOCK_BYTES],
            filled: 0,
        }
    }

    pub(super) fn absorb(&mut self, mut data: &[u8], mut compress: impl FnMut(&[u8; BLOCK_BYTES])) {
        assert!(self.filled < BLOCK_BYTES);
        if self.filled > 0 {
            let taken = data.len().min(BLOCK_BYTES - self.filled);
            self.block[self.filled..self.filled + taken].copy_from_slice(&data[..taken]);
            self.filled += taken;
            data = &data[taken..];
            if self.filled < BLOCK_BYTES {
                return;
            }
            compress(&self.block);
            self.filled = 0;
        }
        let (blocks, rest) = data.as_chunks::<BLOCK_BYTES>();
        for block in blocks {
            compress(block);
        }
        self.block[..rest.len()].copy_from_slice(rest);
        self.filled = rest.len();
    }

    pub(super) fn pad(&mut self, length: &[u8], mut compress: impl FnMut(&[u8; BLOCK_BYTES])) {
        assert!(self.filled < BLOCK_BYTES);
        assert!(length.len() < BLOCK_BYTES);
        self.block[self.filled] = PADDING_MARKER;
        self.block[self.filled + 1..].fill(0);
        if self.filled + 1 > BLOCK_BYTES - length.len() {
            compress(&self.block);
            self.block.fill(0);
        }
        self.block[BLOCK_BYTES - length.len()..].copy_from_slice(length);
        compress(&self.block);
        self.filled = 0;
    }
}
