use std::num::NonZeroUsize;
use std::ops::{Add, Sub};

#[derive(Debug, Eq, PartialEq, Copy, Clone, PartialOrd)]
pub(crate) struct TreeIndex(NonZeroUsize);

impl TreeIndex {
    const fn new(position: usize) -> Self {
        Self(NonZeroUsize::new(position).expect("tree indices start at one"))
    }

    pub const fn get(self) -> usize {
        self.0.get()
    }
}

impl Add<usize> for TreeIndex {
    type Output = Self;

    fn add(self, offset: usize) -> Self {
        let inner = self.0.get() + offset;
        Self::new(inner)
    }
}

impl Sub<usize> for TreeIndex {
    type Output = Self;

    fn sub(self, offset: usize) -> Self {
        let inner = self.0.get().checked_sub(offset).unwrap();
        Self::new(inner)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Node<T> {
    pub child: Option<TreeIndex>,
    pub next: Option<TreeIndex>,
    pub item: T,
}

#[derive(Clone)]
pub(crate) struct Tree<T> {
    nodes: Vec<Node<T>>,
    spine: Vec<TreeIndex>,
    current: Option<TreeIndex>,
}

impl<T: Default> Tree<T> {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        let mut nodes = Vec::with_capacity(capacity);
        nodes.push(Node {
            child: None,
            next: None,
            item: <T as Default>::default(),
        });
        Self {
            nodes,
            spine: Vec::new(),
            current: None,
        }
    }

    pub(crate) const fn current(&self) -> Option<TreeIndex> {
        self.current
    }

    pub(crate) fn append(&mut self, item: T) -> TreeIndex {
        let position = self.create_node(item);
        let this = Some(position);

        if let Some(position) = self.current {
            self[position].next = this;
        } else if let Some(&parent) = self.spine.last() {
            self[parent].child = this;
        }
        self.current = this;
        position
    }

    pub(crate) fn create_node(&mut self, item: T) -> TreeIndex {
        let this = self.nodes.len();
        self.nodes.push(Node {
            child: None,
            next: None,
            item,
        });
        TreeIndex::new(this)
    }

    pub(crate) fn push(&mut self) -> TreeIndex {
        let current_index = self.current.unwrap();
        self.spine.push(current_index);
        self.current = self[current_index].child;
        current_index
    }

    pub(crate) fn pop(&mut self) -> Option<TreeIndex> {
        let position = Some(self.spine.pop()?);
        self.current = position;
        position
    }

    pub(crate) fn remove_node(&mut self) -> Option<TreeIndex> {
        let position = self.spine.pop()?;
        self.current = Some(position);
        self.nodes.pop()?;
        self[position].child = None;
        Some(position)
    }

    pub(crate) fn peek_up(&self) -> Option<TreeIndex> {
        self.spine.last().copied()
    }

    pub(crate) fn peek_grandparent(&self) -> Option<TreeIndex> {
        if self.spine.len() >= 2 {
            Some(self.spine[self.spine.len() - 2])
        } else {
            None
        }
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    pub(crate) const fn spine_length(&self) -> usize {
        self.spine.len()
    }

    pub(crate) fn reset(&mut self) {
        self.current = if self.is_empty() {
            None
        } else {
            Some(TreeIndex::new(1))
        };
        self.spine.clear();
    }

    pub(crate) fn walk_spine(&self) -> impl std::iter::DoubleEndedIterator<Item = &TreeIndex> {
        self.spine.iter()
    }

    pub(crate) fn next_sibling(&mut self, current_index: TreeIndex) -> Option<TreeIndex> {
        self.current = self[current_index].next;
        self.current
    }
}

impl<T> std::ops::Index<TreeIndex> for Tree<T> {
    type Output = Node<T>;

    fn index(&self, position: TreeIndex) -> &Self::Output {
        self.nodes.index(position.get())
    }
}

impl<T> std::ops::IndexMut<TreeIndex> for Tree<T> {
    fn index_mut(&mut self, position: TreeIndex) -> &mut Node<T> {
        self.nodes.index_mut(position.get())
    }
}
