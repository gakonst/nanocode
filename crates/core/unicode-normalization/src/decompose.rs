use std::iter::Fuse;

use crate::lookup::{Form, combining_class, decompose};

pub(crate) struct Decompositions<I> {
    input: Fuse<I>,
    form: Form,
    buffer: Vec<(u8, char)>,
    ready_next: usize,
    ready_end: usize,
}

impl<I: Iterator<Item = char>> Decompositions<I> {
    pub(crate) fn new(input: I, form: Form) -> Self {
        Self {
            input: input.fuse(),
            form,
            buffer: Vec::new(),
            ready_next: 0,
            ready_end: 0,
        }
    }

    fn take_ready(&mut self) -> (u8, char) {
        let entry = self.buffer[self.ready_next];
        self.ready_next += 1;
        if self.ready_next == self.ready_end {
            self.buffer.drain(..self.ready_end);
            self.ready_next = 0;
            self.ready_end = 0;
        }
        entry
    }

    fn release_pending(&mut self) {
        self.buffer[self.ready_end..].sort_by_key(|entry| entry.0);
        self.ready_end = self.buffer.len();
    }

    fn push(&mut self, character: char) {
        let class = combining_class(character);
        if class == 0 {
            self.release_pending();
            self.buffer.push((class, character));
            self.ready_end = self.buffer.len();
        } else {
            self.buffer.push((class, character));
        }
    }
}

impl<I: Iterator<Item = char>> Iterator for Decompositions<I> {
    type Item = (u8, char);

    fn next(&mut self) -> Option<(u8, char)> {
        loop {
            if self.ready_next < self.ready_end {
                return Some(self.take_ready());
            }
            assert_eq!(self.ready_end, 0);
            let Some(character) = self.input.next() else {
                if self.buffer.is_empty() {
                    return None;
                }
                self.release_pending();
                continue;
            };
            if character < self.form.unmapped_starter_below() && self.buffer.is_empty() {
                return Some((0, character));
            }
            decompose(character, self.form, |part| self.push(part));
        }
    }
}
