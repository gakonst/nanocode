use alloc::boxed::Box;
use alloc::vec;

use super::output::Output;

const WINDOW_BYTES: usize = 1 << 15;

#[derive(Clone)]
pub(super) struct Window {
    bytes: Box<[u8]>,
    have: usize,
    next: usize,
}

impl Window {
    pub(super) fn new() -> Self {
        Self {
            bytes: vec![0; WINDOW_BYTES].into_boxed_slice(),
            have: 0,
            next: 0,
        }
    }

    pub(super) const fn have(&self) -> usize {
        self.have
    }

    pub(super) fn extend(&mut self, data: &[u8]) {
        if data.len() >= WINDOW_BYTES {
            self.bytes
                .copy_from_slice(&data[data.len() - WINDOW_BYTES..]);
            self.next = 0;
            self.have = WINDOW_BYTES;
            return;
        }
        let (end_part, start_part) = data.split_at((WINDOW_BYTES - self.next).min(data.len()));
        self.bytes[self.next..self.next + end_part.len()].copy_from_slice(end_part);
        if start_part.is_empty() {
            self.next += end_part.len();
            if self.next == WINDOW_BYTES {
                self.next = 0;
            }
            if self.have < WINDOW_BYTES {
                self.have += end_part.len();
            }
        } else {
            self.bytes[..start_part.len()].copy_from_slice(start_part);
            self.next = start_part.len();
            self.have = WINDOW_BYTES;
        }
        assert!(self.have <= WINDOW_BYTES);
    }

    pub(super) fn copy_to(&self, output: &mut Output<'_>, back: usize, count: usize) {
        assert!(back <= self.have);
        assert!(count <= back);
        let start = if back <= self.next {
            self.next - back
        } else {
            WINDOW_BYTES - (back - self.next)
        };
        let first = count.min(WINDOW_BYTES - start);
        output.extend(&self.bytes[start..start + first]);
        output.extend(&self.bytes[..count - first]);
    }
}
