use std::sync::atomic::{AtomicUsize, Ordering};

use nanocode_sys::{
    AtomicBox,
    queue::{Consumer, Pop, Queue},
};

pub(crate) struct Shared<T> {
    queue: Queue<T>,
    consumer: AtomicBox<Consumer>,
    len: AtomicUsize,
}

impl<T> Shared<T> {
    pub(crate) fn new() -> Self {
        Self {
            queue: Queue::new(),
            consumer: AtomicBox::empty(),
            len: AtomicUsize::new(0),
        }
    }

    pub(crate) fn push(&self, value: T) {
        self.queue.push(value);
        self.len.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn len(&self) -> usize {
        self.len.load(Ordering::SeqCst)
    }

    pub(crate) fn hold(&self) -> Option<Box<Consumer>> {
        self.consumer
            .take()
            .or_else(|| self.queue.consumer().map(Box::new))
    }

    pub(crate) fn give_back(&self, consumer: Box<Consumer>) {
        self.consumer.store(Some(consumer));
    }

    pub(crate) fn pop_held(&self, consumer: &mut Consumer) -> Option<T> {
        loop {
            match self.queue.pop(consumer) {
                Pop::Value(value) => {
                    self.len.fetch_sub(1, Ordering::SeqCst);
                    return Some(value);
                }
                Pop::Pushing => std::hint::spin_loop(),
                Pop::Empty => return None,
            }
        }
    }

    pub(crate) fn pop(&self) -> Option<T> {
        loop {
            if self.len() == 0 {
                return None;
            }
            if let Some(mut consumer) = self.hold() {
                let value = self.pop_held(&mut consumer);
                self.give_back(consumer);
                return value;
            }
            std::hint::spin_loop();
        }
    }

    pub(crate) fn try_drain(&self) -> Vec<T> {
        let mut drained = Vec::new();
        if let Some(mut consumer) = self.hold() {
            while let Some(value) = self.pop_held(&mut consumer) {
                drained.push(value);
            }
            self.give_back(consumer);
        }
        drained
    }

    pub(crate) fn drain(&self) -> Vec<T> {
        let mut drained = Vec::new();
        while let Some(value) = self.pop() {
            drained.push(value);
        }
        drained
    }
}
