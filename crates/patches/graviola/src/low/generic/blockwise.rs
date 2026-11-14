// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

#[derive(Clone)]
pub(crate) struct Blockwise<const N: usize> {
    buffer: [u8; N],
    used: usize,
}

impl<const N: usize> Blockwise<N> {
    pub(crate) const fn new() -> Self {
        Self {
            buffer: [0u8; N],
            used: 0,
        }
    }

    pub(crate) const fn used(&self) -> usize {
        self.used
    }

    pub(crate) fn add(&mut self, bytes: &[u8]) {
        if self.used == 0 {
            self.add_trailing(bytes);
        } else {
            self.add_leading(bytes);
        }
    }

    pub(crate) fn add_leading<'a>(&mut self, bytes: &'a [u8]) -> &'a [u8] {
        if self.used == 0 {
            return bytes;
        }

        let space = N - self.used;
        let take = core::cmp::min(bytes.len(), space);
        let (taken, returned) = bytes.split_at(take);
        self.buffer[self.used..self.used + take].copy_from_slice(taken);
        self.used += take;
        returned
    }

    pub(crate) fn take(&mut self) -> Option<[u8; N]> {
        if self.used == N {
            self.used = 0;
            Some(self.buffer)
        } else {
            None
        }
    }

    pub(crate) fn peek_remaining(&self) -> Option<&[u8]> {
        if self.used > 0 {
            Some(&self.buffer[..self.used])
        } else {
            None
        }
    }

    pub(crate) fn add_trailing(&mut self, trailing: &[u8]) {
        self.buffer[..trailing.len()].copy_from_slice(trailing);
        self.used += trailing.len();
    }

    pub(crate) fn md_pad_with_length(&mut self, length_bits: &[u8]) -> FinalBlocks<N> {
        let space = N - self.used;
        let required = 1 + length_bits.len();

        if required > space {
            // two block case (not especially optimised)
            self.add_leading(&[0x80]);
            self.add_leading(&[0u8; N]);

            let first = self.take().unwrap();
            let mut second = [0u8; N];
            let (_, length) = second.split_at_mut(N - length_bits.len());
            length.copy_from_slice(length_bits);
            FinalBlocks::Two([first, second])
        } else {
            let (_used, trailer) = self.buffer.split_at_mut(self.used);
            let (padding, length) = trailer.split_at_mut(trailer.len() - length_bits.len());
            let (delim, zeroes) = padding.split_at_mut(1);
            delim[0] = 0x80;
            zeroes.fill(0x00);
            length.copy_from_slice(length_bits);
            self.used = 0;
            FinalBlocks::One(self.buffer)
        }
    }
}

pub(crate) enum FinalBlocks<const N: usize> {
    One([u8; N]),
    Two([[u8; N]; 2]),
}

impl<const N: usize> AsRef<[u8]> for FinalBlocks<N> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::One(one) => &one[..],
            Self::Two(two) => two.as_flattened(),
        }
    }
}
