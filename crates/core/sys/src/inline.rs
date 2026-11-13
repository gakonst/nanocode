use std::{
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    ptr, slice,
    sync::Arc,
};

pub struct InlineVec<T, const N: usize> {
    len: usize,
    items: [MaybeUninit<T>; N],
}

impl<T, const N: usize> InlineVec<T, N> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            len: 0,
            items: [const { MaybeUninit::uninit() }; N],
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn as_slice(&self) -> &[T] {
        unsafe { slice::from_raw_parts(self.items.as_ptr().cast::<T>(), self.len) }
    }

    pub const fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { slice::from_raw_parts_mut(self.items.as_mut_ptr().cast::<T>(), self.len) }
    }

    pub fn push(&mut self, item: T) {
        assert!(self.len < N, "InlineVec::push: full at {N} items");
        self.items[self.len].write(item);
        self.len += 1;
    }

    pub const fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { self.items[self.len].assume_init_read() })
    }

    pub fn insert(&mut self, index: usize, item: T) {
        assert!(
            index <= self.len,
            "InlineVec::insert: index past the length"
        );
        self.push(item);
        self.as_mut_slice()[index..].rotate_right(1);
    }

    pub fn remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "InlineVec::remove: index past the length");
        self.as_mut_slice()[index..].rotate_left(1);
        let Some(item) = self.pop() else {
            panic!("InlineVec::remove: a non-empty vector pops")
        };
        item
    }

    #[must_use]
    pub fn split_off(&mut self, at: usize) -> Self {
        assert!(
            at <= self.len,
            "InlineVec::split_off: index past the length"
        );
        let moved = self.len - at;
        let mut rest = Self::new();
        rest.items[..moved].swap_with_slice(&mut self.items[at..self.len]);
        rest.len = moved;
        self.len = at;
        rest
    }

    pub fn append(&mut self, mut other: Self) {
        let moved = other.len;
        assert!(
            self.len + moved <= N,
            "InlineVec::append: more than {N} items"
        );
        self.items[self.len..self.len + moved].swap_with_slice(&mut other.items[..moved]);
        other.len = 0;
        self.len += moved;
    }
}

impl<T: Clone, const N: usize> InlineVec<T, N> {
    pub fn extend_cloned(&mut self, items: &[T]) {
        assert!(
            self.len + items.len() <= N,
            "InlineVec::extend_cloned: more than {N} items"
        );
        for (slot, item) in self.items[self.len..].iter_mut().zip(items) {
            slot.write(item.clone());
        }
        self.len += items.len();
    }
}

impl<T, const N: usize> Default for InlineVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Drop for InlineVec<T, N> {
    fn drop(&mut self) {
        unsafe { ptr::drop_in_place(self.as_mut_slice()) }
    }
}

impl<T: Clone, const N: usize> Clone for InlineVec<T, N> {
    fn clone(&self) -> Self {
        let mut copy = Self::new();
        copy.extend_cloned(self);
        copy
    }
}

impl<T, const N: usize> Deref for InlineVec<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, const N: usize> DerefMut for InlineVec<T, N> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

pub struct OwnedItems<T, const N: usize> {
    items: [MaybeUninit<T>; N],
    front: usize,
    back: usize,
}

impl<T, const N: usize> Iterator for OwnedItems<T, N> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        let item = unsafe { self.items[self.front].assume_init_read() };
        self.front += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.back - self.front;
        (remaining, Some(remaining))
    }
}

impl<T, const N: usize> DoubleEndedIterator for OwnedItems<T, N> {
    fn next_back(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        Some(unsafe { self.items[self.back].assume_init_read() })
    }
}

impl<T, const N: usize> ExactSizeIterator for OwnedItems<T, N> {}

impl<T, const N: usize> Drop for OwnedItems<T, N> {
    fn drop(&mut self) {
        for _ in self.by_ref() {}
    }
}

impl<T, const N: usize> IntoIterator for InlineVec<T, N> {
    type Item = T;
    type IntoIter = OwnedItems<T, N>;

    fn into_iter(mut self) -> OwnedItems<T, N> {
        let items = std::mem::replace(&mut self.items, [const { MaybeUninit::uninit() }; N]);
        let back = self.len;
        self.len = 0;
        OwnedItems {
            items,
            front: 0,
            back,
        }
    }
}

pub fn new_arc_with<T>(make: impl FnOnce() -> T, fill: impl FnOnce(&mut T)) -> Arc<T> {
    let slot = Arc::<T>::new_uninit();
    let place = Arc::as_ptr(&slot).cast_mut();
    let value = unsafe { (*place).write(make()) };
    fill(value);
    unsafe { slot.assume_init() }
}
