const PADDING_START: u8 = 0x80;

#[derive(Clone, Debug)]
pub(crate) struct Pending<const BLOCK_BYTES: usize> {
    bytes: [u8; BLOCK_BYTES],
    filled: usize,
    absorbed_bytes: u128,
}

impl<const BLOCK_BYTES: usize> Pending<BLOCK_BYTES> {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: [0; BLOCK_BYTES],
            filled: 0,
            absorbed_bytes: 0,
        }
    }

    pub(crate) fn absorb(
        &mut self,
        mut input: &[u8],
        mut compress: impl FnMut(&[[u8; BLOCK_BYTES]]),
    ) {
        self.absorbed_bytes = self
            .absorbed_bytes
            .checked_add(input.len() as u128)
            .expect("absorbed bytes fit in u128");
        if self.filled > 0 {
            let taken = (BLOCK_BYTES - self.filled).min(input.len());
            self.bytes[self.filled..self.filled + taken].copy_from_slice(&input[..taken]);
            self.filled += taken;
            input = &input[taken..];
            if self.filled < BLOCK_BYTES {
                return;
            }
            compress(core::slice::from_ref(&self.bytes));
            self.filled = 0;
        }
        let (blocks, rest) = input.as_chunks::<BLOCK_BYTES>();
        if !blocks.is_empty() {
            compress(blocks);
        }
        self.bytes[..rest.len()].copy_from_slice(rest);
        self.filled = rest.len();
    }

    pub(crate) const fn absorbed_bits(&self) -> u128 {
        self.absorbed_bytes
            .checked_mul(u8::BITS as u128)
            .expect("message length in bits fits in u128")
    }

    pub(crate) fn pad(&self, length_be: &[u8], mut compress: impl FnMut(&[[u8; BLOCK_BYTES]])) {
        assert!(self.filled < BLOCK_BYTES);
        assert!(length_be.len() < BLOCK_BYTES);
        let mut block = self.bytes;
        block[self.filled] = PADDING_START;
        block[self.filled + 1..].fill(0);
        if self.filled + 1 > BLOCK_BYTES - length_be.len() {
            compress(core::slice::from_ref(&block));
            block.fill(0);
        }
        block[BLOCK_BYTES - length_be.len()..].copy_from_slice(length_be);
        compress(core::slice::from_ref(&block));
    }
}

pub(crate) fn words_be<const OUTPUT_BYTES: usize, const WORD_BYTES: usize>(
    words: impl IntoIterator<Item = [u8; WORD_BYTES]>,
) -> [u8; OUTPUT_BYTES] {
    let mut output = [0u8; OUTPUT_BYTES];
    let mut written = 0;
    for (slot, word) in output.as_chunks_mut::<WORD_BYTES>().0.iter_mut().zip(words) {
        slot.copy_from_slice(&word);
        written += WORD_BYTES;
    }
    assert_eq!(written, OUTPUT_BYTES);
    output
}
