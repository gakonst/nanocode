use std::{
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::{Index, IndexMut},
    sync::Arc,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeSeq};

use crate::{
    ordered::SequenceVisitor,
    spine::{LEAF_BITS, LEAF_MASK, Leaf, Spine, VECTOR_LEAF_ITEMS, unique_leaf},
    tree::Owned,
};

struct Part<A> {
    leaf: Arc<Leaf<A>>,
    start: usize,
    end: usize,
}

impl<A> Clone for Part<A> {
    fn clone(&self) -> Self {
        Self {
            leaf: Arc::clone(&self.leaf),
            start: self.start,
            end: self.end,
        }
    }
}

impl<A> Part<A> {
    const fn len(&self) -> usize {
        self.end - self.start
    }

    const fn whole(leaf: Arc<Leaf<A>>) -> Self {
        Self {
            leaf,
            start: 0,
            end: VECTOR_LEAF_ITEMS,
        }
    }

    fn single(value: A) -> Self {
        let mut leaf = Leaf::new();
        leaf.slots[0] = Some(value);
        Self {
            leaf: Arc::new(leaf),
            start: 0,
            end: 1,
        }
    }
}

impl<A: Clone> Part<A> {
    fn push(&mut self, value: A) {
        assert!(self.end < VECTOR_LEAF_ITEMS);
        let leaf = unique_leaf(&mut self.leaf, self.start, self.end);
        assert!(leaf.slots[self.end].is_none());
        leaf.slots[self.end] = Some(value);
        self.end += 1;
    }

    fn take(&mut self, index: usize) -> A {
        if self.len() == 1 {
            return match Arc::get_mut(&mut self.leaf) {
                Some(leaf) => leaf.take(index),
                None => self.leaf.item(index).clone(),
            };
        }
        unique_leaf(&mut self.leaf, self.start, self.end).take(index)
    }

    fn take_first(part: &mut Option<Self>) -> A {
        let Some(this) = part else {
            panic!("a non-empty vector has a part to pop")
        };
        let value = this.take(this.start);
        this.start += 1;
        if this.start == this.end {
            *part = None;
        }
        value
    }

    fn take_last(part: &mut Option<Self>) -> A {
        let Some(this) = part else {
            panic!("a non-empty vector has a part to pop")
        };
        let value = this.take(this.end - 1);
        this.end -= 1;
        if this.start == this.end {
            *part = None;
        }
        value
    }

    fn drop_first(part: &mut Option<Self>, count: usize) {
        let Some(this) = part else {
            panic!("a dropped prefix lies in a part")
        };
        assert!(count <= this.len());
        if count == this.len() {
            *part = None;
            return;
        }
        let leaf = unique_leaf(&mut this.leaf, this.start, this.end);
        for index in this.start..this.start + count {
            leaf.take(index);
        }
        this.start += count;
    }

    fn drop_last(part: &mut Option<Self>, count: usize) {
        let Some(this) = part else {
            panic!("a dropped suffix lies in a part")
        };
        assert!(count <= this.len());
        if count == this.len() {
            *part = None;
            return;
        }
        let leaf = unique_leaf(&mut this.leaf, this.start, this.end);
        for index in this.end - count..this.end {
            leaf.take(index);
        }
        this.end -= count;
    }

    fn drain(self, out: &mut Vec<A>) {
        match Arc::try_unwrap(self.leaf) {
            Ok(mut leaf) => {
                for index in self.start..self.end {
                    out.push(leaf.take(index));
                }
            }
            Err(shared) => {
                for index in self.start..self.end {
                    out.push(shared.item(index).clone());
                }
            }
        }
    }
}

pub struct Vector<A> {
    head: Option<Part<A>>,
    middle: Spine<A>,
    tail: Option<Part<A>>,
    len: usize,
}

impl<A> Clone for Vector<A> {
    fn clone(&self) -> Self {
        Self {
            head: self.head.clone(),
            middle: self.middle.clone(),
            tail: self.tail.clone(),
            len: self.len,
        }
    }
}

impl<A> Default for Vector<A> {
    fn default() -> Self {
        Self::new()
    }
}

enum Segment {
    Head(usize),
    Middle(usize),
    Tail(usize),
}

impl<A> Vector<A> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            head: None,
            middle: Spine::new(),
            tail: None,
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    const fn head_len(&self) -> usize {
        match &self.head {
            Some(head) => head.len(),
            None => 0,
        }
    }

    fn segment(&self, index: usize) -> Segment {
        assert!(index < self.len);
        let head_len = self.head_len();
        if index < head_len {
            return Segment::Head(index);
        }
        let index = index - head_len;
        let middle_items = self.middle.len() << LEAF_BITS;
        if index < middle_items {
            return Segment::Middle(index);
        }
        Segment::Tail(index - middle_items)
    }

    fn part(part: Option<&Part<A>>) -> &Part<A> {
        let Some(part) = part else {
            panic!("a position inside the vector lies in a part")
        };
        part
    }

    fn run(&self, index: usize, back: bool) -> &[Option<A>] {
        let (slots, start, end, at) = match self.segment(index) {
            Segment::Head(offset) => {
                let head = Self::part(self.head.as_ref());
                (&head.leaf.slots, head.start, head.end, head.start + offset)
            }
            Segment::Middle(offset) => {
                let leaf = self.middle.leaf(offset >> LEAF_BITS);
                (&leaf.slots, 0, VECTOR_LEAF_ITEMS, offset & LEAF_MASK)
            }
            Segment::Tail(offset) => {
                let tail = Self::part(self.tail.as_ref());
                (&tail.leaf.slots, tail.start, tail.end, tail.start + offset)
            }
        };
        if back {
            &slots[start..=at]
        } else {
            &slots[at..end]
        }
    }

    pub const fn iter(&self) -> Iter<'_, A> {
        Iter {
            vector: self,
            front: &[],
            back: &[],
            start: 0,
            end: self.len,
        }
    }

    pub fn get(&self, index: usize) -> Option<&A> {
        if index >= self.len {
            return None;
        }
        Some(match self.segment(index) {
            Segment::Head(offset) => {
                let head = Self::part(self.head.as_ref());
                head.leaf.item(head.start + offset)
            }
            Segment::Middle(offset) => self
                .middle
                .leaf(offset >> LEAF_BITS)
                .item(offset & LEAF_MASK),
            Segment::Tail(offset) => {
                let tail = Self::part(self.tail.as_ref());
                tail.leaf.item(tail.start + offset)
            }
        })
    }

    pub fn front(&self) -> Option<&A> {
        self.get(0)
    }

    pub fn back(&self) -> Option<&A> {
        self.get(self.len.checked_sub(1)?)
    }

    pub fn last(&self) -> Option<&A> {
        self.back()
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

impl<A: Clone> Vector<A> {
    #[must_use]
    pub fn skip(&self, count: usize) -> Self {
        if count >= self.len() {
            return Self::new();
        }
        let mut vector = self.clone();
        vector.drop_front(count);
        vector
    }

    #[must_use]
    pub fn take(&self, count: usize) -> Self {
        assert!(count <= self.len(), "Vector::take: count beyond the length");
        let mut vector = self.clone();
        vector.drop_back(self.len() - count);
        vector
    }

    fn drop_front(&mut self, count: usize) {
        assert!(count <= self.len);
        let mut left = count;
        for _ in 0..=self.middle.len() + 2 {
            if left == 0 {
                self.len -= count;
                return;
            }
            if let Some(head) = &self.head {
                let dropped = left.min(head.len());
                Part::drop_first(&mut self.head, dropped);
                left -= dropped;
            } else if !self.middle.is_empty() {
                let leaf = self.middle.pop_first();
                if left >= VECTOR_LEAF_ITEMS {
                    left -= VECTOR_LEAF_ITEMS;
                } else {
                    self.head = Some(Part::whole(leaf));
                }
            } else {
                let dropped = left.min(Self::part(self.tail.as_ref()).len());
                Part::drop_first(&mut self.tail, dropped);
                left -= dropped;
            }
        }
        panic!("dropping a prefix walks each part once")
    }

    fn drop_back(&mut self, count: usize) {
        assert!(count <= self.len);
        let mut left = count;
        for _ in 0..=self.middle.len() + 2 {
            if left == 0 {
                self.len -= count;
                return;
            }
            if let Some(tail) = &self.tail {
                let dropped = left.min(tail.len());
                Part::drop_last(&mut self.tail, dropped);
                left -= dropped;
            } else if !self.middle.is_empty() {
                let leaf = self.middle.pop_last();
                if left >= VECTOR_LEAF_ITEMS {
                    left -= VECTOR_LEAF_ITEMS;
                } else {
                    self.tail = Some(Part::whole(leaf));
                }
            } else {
                let dropped = left.min(Self::part(self.head.as_ref()).len());
                Part::drop_last(&mut self.head, dropped);
                left -= dropped;
            }
        }
        panic!("dropping a suffix walks each part once")
    }

    pub fn push_back(&mut self, value: A) {
        self.len += 1;
        match &mut self.tail {
            Some(tail) if tail.end < VECTOR_LEAF_ITEMS => {
                tail.push(value);
                return;
            }
            Some(_) => {
                let Some(full) = self.tail.take() else {
                    panic!("a full tail is present")
                };
                if self.head.is_none() && self.middle.is_empty() {
                    self.head = Some(full);
                } else {
                    assert!(full.start == 0);
                    self.middle.push_last(full.leaf);
                }
            }
            None => {
                if let Some(head) = &mut self.head
                    && self.middle.is_empty()
                    && head.end < VECTOR_LEAF_ITEMS
                {
                    head.push(value);
                    return;
                }
            }
        }
        self.tail = Some(Part::single(value));
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&A) -> bool) {
        let mut kept = Self::new();
        for item in self.iter() {
            if keep(item) {
                kept.push_back(item.clone());
            }
        }
        if kept.len() < self.len() {
            *self = kept;
        }
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut A> {
        if index >= self.len {
            return None;
        }
        Some(match self.segment(index) {
            Segment::Head(offset) => {
                let Some(head) = self.head.as_mut() else {
                    panic!("a position inside the vector lies in a part")
                };
                unique_leaf(&mut head.leaf, head.start, head.end).item_mut(head.start + offset)
            }
            Segment::Middle(offset) => self
                .middle
                .leaf_mut(offset >> LEAF_BITS)
                .item_mut(offset & LEAF_MASK),
            Segment::Tail(offset) => {
                let Some(tail) = self.tail.as_mut() else {
                    panic!("a position inside the vector lies in a part")
                };
                unique_leaf(&mut tail.leaf, tail.start, tail.end).item_mut(tail.start + offset)
            }
        })
    }

    pub fn pop_front(&mut self) -> Option<A> {
        if self.len == 0 {
            return None;
        }
        if self.head.is_none() && !self.middle.is_empty() {
            self.head = Some(Part::whole(self.middle.pop_first()));
        }
        let value = if self.head.is_some() {
            Part::take_first(&mut self.head)
        } else {
            Part::take_first(&mut self.tail)
        };
        self.len -= 1;
        Some(value)
    }

    pub fn pop_back(&mut self) -> Option<A> {
        if self.len == 0 {
            return None;
        }
        if self.tail.is_none() && !self.middle.is_empty() {
            self.tail = Some(Part::whole(self.middle.pop_last()));
        }
        let value = if self.tail.is_some() {
            Part::take_last(&mut self.tail)
        } else {
            Part::take_last(&mut self.head)
        };
        self.len -= 1;
        Some(value)
    }
}

pub struct Iter<'a, A> {
    vector: &'a Vector<A>,
    front: &'a [Option<A>],
    back: &'a [Option<A>],
    start: usize,
    end: usize,
}

impl<A> Clone for Iter<'_, A> {
    fn clone(&self) -> Self {
        Self {
            vector: self.vector,
            front: self.front,
            back: self.back,
            start: self.start,
            end: self.end,
        }
    }
}

const fn live<A>(slot: &Option<A>) -> &A {
    let Some(item) = slot else {
        panic!("a live slot holds an item")
    };
    item
}

impl<'a, A> Iterator for Iter<'a, A> {
    type Item = &'a A;

    fn next(&mut self) -> Option<&'a A> {
        if self.start >= self.end {
            return None;
        }
        if self.front.is_empty() {
            self.front = self.vector.run(self.start, false);
        }
        let Some((first, rest)) = self.front.split_first() else {
            panic!("a run inside the vector holds an item")
        };
        self.front = rest;
        self.start += 1;
        Some(live(first))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.end - self.start;
        (remaining, Some(remaining))
    }
}

impl<'a, A> DoubleEndedIterator for Iter<'a, A> {
    fn next_back(&mut self) -> Option<&'a A> {
        if self.start >= self.end {
            return None;
        }
        if self.back.is_empty() {
            self.back = self.vector.run(self.end - 1, true);
        }
        let Some((last, rest)) = self.back.split_last() else {
            panic!("a run inside the vector holds an item")
        };
        self.back = rest;
        self.end -= 1;
        Some(live(last))
    }
}

impl<A> ExactSizeIterator for Iter<'_, A> {}

impl<A> std::iter::FusedIterator for Iter<'_, A> {}

impl<A: PartialEq> PartialEq for Vector<A> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<A: Eq> Eq for Vector<A> {}

impl<A: Hash> Hash for Vector<A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for item in self.iter() {
            item.hash(state);
        }
    }
}

impl<A: fmt::Debug> fmt::Debug for Vector<A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.iter()).finish()
    }
}

impl<A: Clone> Extend<A> for Vector<A> {
    fn extend<I: IntoIterator<Item = A>>(&mut self, values: I) {
        for value in values {
            self.push_back(value);
        }
    }
}

impl<A: Clone> FromIterator<A> for Vector<A> {
    fn from_iter<I: IntoIterator<Item = A>>(values: I) -> Self {
        let mut vector = Self::new();
        vector.extend(values);
        vector
    }
}

impl<A> Index<usize> for Vector<A> {
    type Output = A;

    fn index(&self, index: usize) -> &A {
        let Some(item) = self.get(index) else {
            panic!(
                "Vector::index: index out of bounds: {index} >= {}",
                self.len()
            )
        };
        item
    }
}

impl<A: Clone> IndexMut<usize> for Vector<A> {
    fn index_mut(&mut self, index: usize) -> &mut A {
        let len = self.len();
        let Some(item) = self.get_mut(index) else {
            panic!("Vector::index_mut: index out of bounds: {index} >= {len}")
        };
        item
    }
}

impl<'a, A> IntoIterator for &'a Vector<A> {
    type Item = &'a A;
    type IntoIter = Iter<'a, A>;

    fn into_iter(self) -> Iter<'a, A> {
        self.iter()
    }
}

impl<A: Clone> IntoIterator for Vector<A> {
    type Item = A;
    type IntoIter = Owned<A>;

    fn into_iter(mut self) -> Owned<A> {
        let mut items = Vec::with_capacity(self.len);
        if let Some(head) = self.head.take() {
            head.drain(&mut items);
        }
        for _ in 0..self.middle.len() {
            Part::whole(self.middle.pop_first()).drain(&mut items);
        }
        if let Some(tail) = self.tail.take() {
            tail.drain(&mut items);
        }
        assert!(items.len() == self.len);
        Owned::from_items(items)
    }
}

impl<A: Serialize> Serialize for Vector<A> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.len()))?;
        for item in self.iter() {
            sequence.serialize_element(item)?;
        }
        sequence.end()
    }
}

impl<'de, A: Deserialize<'de> + Clone> Deserialize<'de> for Vector<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let items = deserializer.deserialize_seq(SequenceVisitor(PhantomData))?;
        Ok(items.into_iter().collect())
    }
}
