use std::cell::{Cell, OnceCell};

const CHUNKS: usize = usize::BITS as usize;

pub(crate) struct AppendList<T> {
    chunks: [OnceCell<Box<[OnceCell<T>]>>; CHUNKS],
    length: Cell<usize>,
}

const fn locate(index: usize) -> (usize, usize) {
    let position = index + 1;
    let chunk = (usize::BITS - 1 - position.leading_zeros()) as usize;
    (chunk, position - (1 << chunk))
}

impl<T> AppendList<T> {
    pub(crate) fn new() -> Self {
        Self {
            chunks: std::array::from_fn(|_| OnceCell::new()),
            length: Cell::new(0),
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.length.get()
    }

    pub(crate) fn push(&self, value: T) {
        let index = self.length.get();
        let (chunk, offset) = locate(index);
        let slots = self.chunks[chunk]
            .get_or_init(|| (0..1usize << chunk).map(|_| OnceCell::new()).collect());
        let stored = slots[offset].set(value);
        assert!(stored.is_ok());
        self.length.set(index + 1);
    }

    pub(crate) fn get(&self, index: usize) -> Option<&T> {
        if index >= self.length.get() {
            return None;
        }
        let (chunk, offset) = locate(index);
        self.chunks[chunk].get()?[offset].get()
    }
}
