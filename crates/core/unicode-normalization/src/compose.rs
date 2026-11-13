use std::mem;

use crate::{
    decompose::CanonicalOrder,
    lookup::{Form, compose},
};

pub struct Recompositions<I> {
    decompositions: CanonicalOrder<I>,
    composee: Option<char>,
    pending: Vec<(u8, char)>,
    ready: Vec<(u8, char)>,
    ready_next: usize,
}

impl<I: Iterator<Item = char>> Recompositions<I> {
    pub(crate) fn new(input: I, form: Form) -> Self {
        Self {
            decompositions: CanonicalOrder::new(input, form),
            composee: None,
            pending: Vec::new(),
            ready: Vec::new(),
            ready_next: 0,
        }
    }

    fn release_pending(&mut self) {
        assert_eq!(self.ready_next, self.ready.len());
        self.ready.clear();
        mem::swap(&mut self.ready, &mut self.pending);
        self.ready_next = 0;
    }
}

impl<I: Iterator<Item = char>> Iterator for Recompositions<I> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        loop {
            if let Some(&(_, character)) = self.ready.get(self.ready_next) {
                self.ready_next += 1;
                return Some(character);
            }
            let Some((class, character)) = self.decompositions.next() else {
                let Some(composee) = self.composee.take() else {
                    assert!(self.pending.is_empty());
                    return None;
                };
                self.release_pending();
                return Some(composee);
            };
            let Some(composee) = self.composee else {
                assert!(self.pending.is_empty());
                if class != 0 {
                    return Some(character);
                }
                self.composee = Some(character);
                continue;
            };
            let blocked = self.pending.last().is_some_and(|&(last, _)| last >= class);
            if !blocked && let Some(composite) = compose(composee, character) {
                self.composee = Some(composite);
                continue;
            }
            if class == 0 {
                self.composee = Some(character);
                self.release_pending();
                return Some(composee);
            }
            self.pending.push((class, character));
        }
    }
}
