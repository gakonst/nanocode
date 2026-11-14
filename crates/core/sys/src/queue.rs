use std::{
    cell::UnsafeCell,
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

struct Node<T> {
    next: AtomicPtr<Self>,
    value: Option<T>,
}

impl<T> Node<T> {
    fn boxed(value: Option<T>) -> *mut Self {
        Box::into_raw(Box::new(Self {
            next: AtomicPtr::new(ptr::null_mut()),
            value,
        }))
    }
}

pub enum Pop<T> {
    Value(T),
    Empty,
    Pushing,
}

pub struct Queue<T> {
    head: AtomicPtr<Node<T>>,
    tail: UnsafeCell<*mut Node<T>>,
    consumed: AtomicBool,
}

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

pub struct Consumer {
    queue: usize,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Queue<T> {
    #[must_use]
    pub fn new() -> Self {
        let stub = Node::boxed(None);
        Self {
            head: AtomicPtr::new(stub),
            tail: UnsafeCell::new(stub),
            consumed: AtomicBool::new(false),
        }
    }

    fn address(&self) -> usize {
        ptr::from_ref(self).addr()
    }

    pub fn consumer(&self) -> Option<Consumer> {
        (!self.consumed.swap(true, Ordering::AcqRel)).then(|| Consumer {
            queue: self.address(),
        })
    }

    pub fn push(&self, value: T) {
        let node = Node::boxed(Some(value));
        let previous = self.head.swap(node, Ordering::AcqRel);
        unsafe { (*previous).next.store(node, Ordering::Release) };
    }

    pub fn pop(&self, consumer: &mut Consumer) -> Pop<T> {
        assert_eq!(
            consumer.queue,
            self.address(),
            "a consumer pops only its own queue"
        );
        let tail = unsafe { *self.tail.get() };
        let next = unsafe { (*tail).next.load(Ordering::Acquire) };
        if next.is_null() {
            if self.head.load(Ordering::Acquire) == tail {
                return Pop::Empty;
            }
            return Pop::Pushing;
        }
        unsafe {
            *self.tail.get() = next;
            drop(Box::from_raw(tail));
            Pop::Value(
                (*next)
                    .value
                    .take()
                    .expect("a pushed node carries its value until it is popped"),
            )
        }
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        let mut node = *self.tail.get_mut();
        while !node.is_null() {
            let next = unsafe { (*node).next.load(Ordering::Acquire) };
            drop(unsafe { Box::from_raw(node) });
            node = next;
        }
    }
}
