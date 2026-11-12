use std::{
    borrow::Borrow,
    cmp::Ordering,
    ops::{Bound, RangeBounds},
    sync::Arc,
};

const NODE_ITEMS_MIN: usize = 8;
const NODE_ITEMS_MAX: usize = 2 * NODE_ITEMS_MIN + 1;
const HEIGHT_MAX: usize = usize::BITS as usize;

pub(crate) struct Node<T> {
    items: Vec<T>,
    children: Vec<Child<T>>,
}

struct Child<T> {
    count: usize,
    node: Arc<Node<T>>,
}

impl<T> Clone for Child<T> {
    fn clone(&self) -> Self {
        Self {
            count: self.count,
            node: Arc::clone(&self.node),
        }
    }
}

impl<T: Clone> Clone for Node<T> {
    fn clone(&self) -> Self {
        let mut items = Vec::with_capacity(NODE_ITEMS_MAX + 1);
        items.extend(self.items.iter().cloned());
        let children = if self.children.is_empty() {
            Vec::new()
        } else {
            let mut children = Vec::with_capacity(NODE_ITEMS_MAX + 2);
            children.extend(self.children.iter().cloned());
            children
        };
        Self { items, children }
    }
}

pub(crate) enum Place {
    Found(usize),
    Below(usize),
}

pub(crate) trait Locate<T> {
    fn locate(&mut self, node: &Node<T>) -> Place;
}

pub(crate) trait Slot<T> {
    fn slot(&mut self, node: &Node<T>, item: &T) -> Place;
}

pub(crate) struct ByKey<'a, Q: ?Sized, F> {
    pub(crate) key: &'a Q,
    pub(crate) project: F,
}

impl<T, K, Q, F> Locate<T> for ByKey<'_, Q, F>
where
    K: Borrow<Q>,
    Q: Ord + ?Sized,
    F: Fn(&T) -> &K,
{
    fn locate(&mut self, node: &Node<T>) -> Place {
        match node
            .items
            .binary_search_by(|item| (self.project)(item).borrow().cmp(self.key))
        {
            Ok(index) => Place::Found(index),
            Err(index) => Place::Below(index),
        }
    }
}

pub(crate) struct ByOrder<F>(pub(crate) F);

impl<T, K, F> Slot<T> for ByOrder<F>
where
    K: Ord,
    F: Fn(&T) -> &K,
{
    fn slot(&mut self, node: &Node<T>, item: &T) -> Place {
        let key = (self.0)(item);
        match node
            .items
            .binary_search_by(|existing| (self.0)(existing).cmp(key))
        {
            Ok(index) => Place::Found(index),
            Err(index) => Place::Below(index),
        }
    }
}

pub(crate) struct At(pub(crate) usize);

impl<T> Locate<T> for At {
    fn locate(&mut self, node: &Node<T>) -> Place {
        if node.is_leaf() {
            assert!(self.0 < node.items.len());
            return Place::Found(self.0);
        }
        for (index, child) in node.children.iter().enumerate() {
            match self.0.cmp(&child.count) {
                Ordering::Less => return Place::Below(index),
                Ordering::Equal => return Place::Found(index),
                Ordering::Greater => self.0 -= child.count + 1,
            }
        }
        panic!("position beyond the subtree's count")
    }
}

enum Grown<T> {
    Replaced(T),
    Added,
    Split(T, Node<T>),
}

impl<T> Node<T> {
    fn count(&self) -> usize {
        self.items.len() + self.children.iter().map(|child| child.count).sum::<usize>()
    }

    const fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

impl<T: Clone> Node<T> {
    fn leaf(item: T) -> Self {
        let mut items = Vec::with_capacity(NODE_ITEMS_MAX + 1);
        items.push(item);
        Self {
            items,
            children: Vec::new(),
        }
    }

    fn split(&mut self, rightmost: bool) -> (T, Self) {
        assert!(self.items.len() > NODE_ITEMS_MAX);
        let middle = if rightmost {
            self.items.len() - 2
        } else {
            self.items.len() / 2
        };
        let mut items = Vec::with_capacity(NODE_ITEMS_MAX + 1);
        items.extend(self.items.drain(middle + 1..));
        let Some(median) = self.items.pop() else {
            panic!("a full node has a median")
        };
        let children = if self.is_leaf() {
            Vec::new()
        } else {
            let mut children = Vec::with_capacity(NODE_ITEMS_MAX + 2);
            children.extend(self.children.drain(middle + 1..));
            children
        };
        (median, Self { items, children })
    }

    fn adopt(&mut self, index: usize, median: T, right: Self) {
        let right_count = right.count();
        self.children[index].count -= right_count + 1;
        self.items.insert(index, median);
        self.children.insert(
            index + 1,
            Child {
                count: right_count,
                node: Arc::new(right),
            },
        );
    }

    fn insert(&mut self, item: T, slot: &mut impl Slot<T>, rightmost: bool) -> Grown<T> {
        let index = match slot.slot(self, &item) {
            Place::Found(index) => {
                return Grown::Replaced(std::mem::replace(&mut self.items[index], item));
            }
            Place::Below(index) => index,
        };
        if self.is_leaf() {
            self.items.insert(index, item);
        } else {
            let last = index + 1 == self.children.len();
            let child = &mut self.children[index];
            match Arc::make_mut(&mut child.node).insert(item, slot, rightmost && last) {
                Grown::Replaced(old) => return Grown::Replaced(old),
                Grown::Added => {
                    child.count += 1;
                    return Grown::Added;
                }
                Grown::Split(median, right) => {
                    child.count += 1;
                    self.adopt(index, median, right);
                }
            }
        }
        if self.items.len() > NODE_ITEMS_MAX {
            let at_end = index + 1 == self.items.len();
            let (median, right) = self.split(rightmost && at_end);
            return Grown::Split(median, right);
        }
        Grown::Added
    }

    fn push_last(&mut self, item: T) -> Option<(T, Self)> {
        if self.is_leaf() {
            self.items.push(item);
        } else {
            let index = self.children.len() - 1;
            let child = &mut self.children[index];
            let grown = Arc::make_mut(&mut child.node).push_last(item);
            child.count += 1;
            if let Some((median, right)) = grown {
                self.adopt(index, median, right);
            }
        }
        if self.items.len() > NODE_ITEMS_MAX {
            return Some(self.split(true));
        }
        None
    }

    fn remove(&mut self, locate: &mut impl Locate<T>) -> T {
        match locate.locate(self) {
            Place::Found(index) if self.is_leaf() => self.items.remove(index),
            Place::Found(index) => {
                let child = &mut self.children[index];
                let predecessor = Arc::make_mut(&mut child.node).remove_last();
                child.count -= 1;
                let old = std::mem::replace(&mut self.items[index], predecessor);
                self.rebalance(index);
                old
            }
            Place::Below(index) => {
                assert!(!self.is_leaf());
                let child = &mut self.children[index];
                let old = Arc::make_mut(&mut child.node).remove(locate);
                child.count -= 1;
                self.rebalance(index);
                old
            }
        }
    }

    fn remove_first(&mut self) -> T {
        if self.is_leaf() {
            assert!(!self.items.is_empty());
            return self.items.remove(0);
        }
        let child = &mut self.children[0];
        let first = Arc::make_mut(&mut child.node).remove_first();
        child.count -= 1;
        self.rebalance(0);
        first
    }

    fn remove_last(&mut self) -> T {
        if self.is_leaf() {
            let Some(last) = self.items.pop() else {
                panic!("a non-root node holds items")
            };
            return last;
        }
        let index = self.children.len() - 1;
        let child = &mut self.children[index];
        let last = Arc::make_mut(&mut child.node).remove_last();
        child.count -= 1;
        self.rebalance(index);
        last
    }

    fn rebalance(&mut self, index: usize) {
        if self.children[index].node.items.len() >= NODE_ITEMS_MIN {
            return;
        }
        let held = self.children[index].node.items.len();
        if index > 0 && self.children[index - 1].node.items.len() > NODE_ITEMS_MIN {
            let lent = Self::lent(self.children[index - 1].node.items.len(), held);
            for _ in 0..lent {
                self.rotate_right(index);
            }
        } else if index + 1 < self.children.len()
            && self.children[index + 1].node.items.len() > NODE_ITEMS_MIN
        {
            let lent = Self::lent(self.children[index + 1].node.items.len(), held);
            for _ in 0..lent {
                self.rotate_left(index);
            }
        } else if index > 0 {
            self.merge(index - 1);
        } else {
            self.merge(index);
        }
    }

    fn lent(lender: usize, held: usize) -> usize {
        assert!(lender > NODE_ITEMS_MIN);
        assert!(held < NODE_ITEMS_MIN);
        ((lender - held) / 2).min(lender - NODE_ITEMS_MIN)
    }

    fn rotate_right(&mut self, index: usize) {
        let (before, after) = self.children.split_at_mut(index);
        let left = &mut before[index - 1];
        let right = &mut after[0];
        let left_node = Arc::make_mut(&mut left.node);
        let Some(borrowed) = left_node.items.pop() else {
            panic!("a lending sibling holds items")
        };
        let moved_child = left_node.children.pop();
        let separator = std::mem::replace(&mut self.items[index - 1], borrowed);
        let right_node = Arc::make_mut(&mut right.node);
        right_node.items.insert(0, separator);
        let moved = 1 + moved_child.as_ref().map_or(0, |child| child.count);
        if let Some(child) = moved_child {
            right_node.children.insert(0, child);
        }
        left.count -= moved;
        right.count += moved;
    }

    fn rotate_left(&mut self, index: usize) {
        let (before, after) = self.children.split_at_mut(index + 1);
        let left = &mut before[index];
        let right = &mut after[0];
        let right_node = Arc::make_mut(&mut right.node);
        let borrowed = right_node.items.remove(0);
        let moved_child = if right_node.is_leaf() {
            None
        } else {
            Some(right_node.children.remove(0))
        };
        let separator = std::mem::replace(&mut self.items[index], borrowed);
        let left_node = Arc::make_mut(&mut left.node);
        left_node.items.push(separator);
        let moved = 1 + moved_child.as_ref().map_or(0, |child| child.count);
        if let Some(child) = moved_child {
            left_node.children.push(child);
        }
        left.count += moved;
        right.count -= moved;
    }

    fn merge(&mut self, index: usize) {
        let separator = self.items.remove(index);
        let right = self.children.remove(index + 1);
        let left = &mut self.children[index];
        let left_node = Arc::make_mut(&mut left.node);
        left_node.items.push(separator);
        match Arc::try_unwrap(right.node) {
            Ok(right_node) => {
                left_node.items.extend(right_node.items);
                left_node.children.extend(right_node.children);
            }
            Err(shared) => {
                left_node.items.extend(shared.items.iter().cloned());
                left_node.children.extend(shared.children.iter().cloned());
            }
        }
        assert!(left_node.items.len() <= NODE_ITEMS_MAX);
        left.count += 1 + right.count;
    }
}

pub(crate) struct Tree<T> {
    root: Option<Arc<Node<T>>>,
    count: usize,
}

impl<T> Clone for Tree<T> {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            count: self.count,
        }
    }
}

impl<T> Tree<T> {
    pub(crate) const fn new() -> Self {
        Self {
            root: None,
            count: 0,
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.count
    }

    pub(crate) fn find(&self, mut locate: impl Locate<T>) -> Option<&T> {
        let mut node = self.root.as_deref()?;
        for _ in 0..HEIGHT_MAX {
            match locate.locate(node) {
                Place::Found(index) => return Some(&node.items[index]),
                Place::Below(_) if node.is_leaf() => return None,
                Place::Below(index) => node = &node.children[index].node,
            }
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    pub(crate) fn rank<K, Q>(&self, key: &Q, inclusive: bool, project: &impl Fn(&T) -> &K) -> usize
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let mut rank = 0;
        let Some(mut node) = self.root.as_deref() else {
            return 0;
        };
        for _ in 0..HEIGHT_MAX {
            let below = node
                .items
                .partition_point(|item| match project(item).borrow().cmp(key) {
                    Ordering::Less => true,
                    Ordering::Equal => inclusive,
                    Ordering::Greater => false,
                });
            rank += below;
            if node.is_leaf() {
                return rank;
            }
            rank += node.children[..below]
                .iter()
                .map(|child| child.count)
                .sum::<usize>();
            node = &node.children[below].node;
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    pub(crate) fn range<K, Q>(
        &self,
        range: &impl RangeBounds<Q>,
        project: impl Fn(&T) -> &K,
    ) -> Items<'_, T>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let start = match range.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(key) => self.rank(key, false, &project),
            Bound::Excluded(key) => self.rank(key, true, &project),
        };
        let end = match range.end_bound() {
            Bound::Unbounded => self.count,
            Bound::Included(key) => self.rank(key, true, &project),
            Bound::Excluded(key) => self.rank(key, false, &project),
        };
        self.items(start, end.max(start))
    }

    pub(crate) fn items(&self, start: usize, end: usize) -> Items<'_, T> {
        assert!(start <= end);
        assert!(end <= self.count);
        Items {
            root: self.root.as_deref(),
            front: Vec::new(),
            back: Vec::new(),
            start,
            end,
        }
    }
}

impl<T: Clone> Tree<T> {
    pub(crate) fn find_mut(&mut self, mut locate: impl Locate<T>) -> Option<&mut T> {
        let mut node = Arc::make_mut(self.root.as_mut()?);
        for _ in 0..HEIGHT_MAX {
            match locate.locate(node) {
                Place::Found(index) => return Some(&mut node.items[index]),
                Place::Below(_) if node.is_leaf() => return None,
                Place::Below(index) => node = Arc::make_mut(&mut node.children[index].node),
            }
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    fn grow(&mut self, median: T, right: Node<T>) {
        let Some(root) = self.root.as_mut() else {
            panic!("a split needs a root")
        };
        let right_count = right.count();
        let left_count = self.count - right_count - 1;
        let mut items = Vec::with_capacity(NODE_ITEMS_MAX + 1);
        items.push(median);
        let mut children = Vec::with_capacity(NODE_ITEMS_MAX + 2);
        children.push(Child {
            count: left_count,
            node: Arc::clone(root),
        });
        children.push(Child {
            count: right_count,
            node: Arc::new(right),
        });
        *root = Arc::new(Node { items, children });
    }

    pub(crate) fn insert(&mut self, item: T, mut slot: impl Slot<T>) -> Option<T> {
        let Some(root) = self.root.as_mut() else {
            assert!(self.count == 0);
            self.root = Some(Arc::new(Node::leaf(item)));
            self.count = 1;
            return None;
        };
        match Arc::make_mut(root).insert(item, &mut slot, true) {
            Grown::Replaced(old) => return Some(old),
            Grown::Added => self.count += 1,
            Grown::Split(median, right) => {
                self.count += 1;
                self.grow(median, right);
            }
        }
        None
    }

    pub(crate) fn push(&mut self, item: T) {
        let Some(root) = self.root.as_mut() else {
            assert!(self.count == 0);
            self.root = Some(Arc::new(Node::leaf(item)));
            self.count = 1;
            return;
        };
        let grown = Arc::make_mut(root).push_last(item);
        self.count += 1;
        if let Some((median, right)) = grown {
            self.grow(median, right);
        }
    }

    fn shrink(&mut self, removed: T) -> T {
        let Some(root) = self.root.as_mut() else {
            panic!("a removal had a root")
        };
        let root_node = Arc::make_mut(root);
        if root_node.items.is_empty() {
            self.root = root_node.children.pop().map(|child| child.node);
        }
        self.count -= 1;
        assert!(self.count > 0 || self.root.is_none());
        removed
    }

    pub(crate) fn remove(&mut self, mut locate: impl Locate<T>) -> T {
        let Some(root) = self.root.as_mut() else {
            panic!("removal from an empty tree")
        };
        let removed = Arc::make_mut(root).remove(&mut locate);
        self.shrink(removed)
    }

    pub(crate) fn pop_first(&mut self) -> Option<T> {
        let root = self.root.as_mut()?;
        let removed = Arc::make_mut(root).remove_first();
        Some(self.shrink(removed))
    }

    pub(crate) fn pop_last(&mut self) -> Option<T> {
        let root = self.root.as_mut()?;
        let removed = Arc::make_mut(root).remove_last();
        Some(self.shrink(removed))
    }

    pub(crate) fn slice(&self, start: usize, end: usize) -> Self {
        assert!(start <= end);
        assert!(end <= self.count);
        let kept = end - start;
        if self.count - kept <= kept {
            let mut tree = self.clone();
            for _ in end..self.count {
                tree.pop_last();
            }
            for _ in 0..start {
                tree.pop_first();
            }
            assert!(tree.count == kept);
            return tree;
        }
        let mut tree = Self::new();
        for item in self.items(start, end) {
            tree.push(item.clone());
        }
        tree
    }
}

type Frame<'a, T> = (&'a Node<T>, usize);

pub struct Items<'a, T> {
    root: Option<&'a Node<T>>,
    front: Vec<Frame<'a, T>>,
    back: Vec<Frame<'a, T>>,
    start: usize,
    end: usize,
}

impl<T> Clone for Items<'_, T> {
    fn clone(&self) -> Self {
        Self {
            root: self.root,
            front: self.front.clone(),
            back: self.back.clone(),
            start: self.start,
            end: self.end,
        }
    }
}

impl<'a, T> Items<'a, T> {
    fn root(&self) -> &'a Node<T> {
        let Some(root) = self.root else {
            panic!("a non-empty range has a root")
        };
        root
    }

    fn seek(&mut self, position: usize, back: bool) {
        let mut node = self.root();
        let mut position = position;
        let stack = if back {
            &mut self.back
        } else {
            &mut self.front
        };
        let after = usize::from(back);
        'descend: for _ in 0..HEIGHT_MAX {
            if node.is_leaf() {
                stack.push((node, position + after));
                return;
            }
            for (index, child) in node.children.iter().enumerate() {
                match position.cmp(&child.count) {
                    Ordering::Less => {
                        stack.push((node, index));
                        node = &child.node;
                        continue 'descend;
                    }
                    Ordering::Equal => {
                        stack.push((node, index + after));
                        return;
                    }
                    Ordering::Greater => position -= child.count + 1,
                }
            }
            panic!("position beyond the subtree's count")
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    fn step_front(&mut self) -> &'a T {
        let Some(top) = self.front.last_mut() else {
            panic!("a front cursor inside the range")
        };
        let (node, index) = *top;
        top.1 = index + 1;
        if node.is_leaf() {
            while let Some(&(node, index)) = self.front.last() {
                if index < node.items.len() {
                    break;
                }
                self.front.pop();
            }
        } else {
            let mut below: &'a Node<T> = &node.children[index + 1].node;
            self.front.push((below, 0));
            while !below.is_leaf() {
                below = &below.children[0].node;
                self.front.push((below, 0));
            }
        }
        &node.items[index]
    }

    fn step_back(&mut self) -> &'a T {
        let Some(top) = self.back.last_mut() else {
            panic!("a back cursor inside the range")
        };
        let (node, end) = *top;
        let index = end - 1;
        top.1 = index;
        if node.is_leaf() {
            while let Some(&(_, end)) = self.back.last() {
                if end > 0 {
                    break;
                }
                self.back.pop();
            }
        } else {
            let mut below: &'a Node<T> = &node.children[index].node;
            self.back.push((below, below.items.len()));
            while !below.is_leaf() {
                below = &below.children[below.children.len() - 1].node;
                self.back.push((below, below.items.len()));
            }
        }
        &node.items[index]
    }

    const fn remaining(&self) -> usize {
        self.end - self.start
    }
}

impl<'a, T> Iterator for Items<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        if self.start >= self.end {
            return None;
        }
        if self.front.is_empty() {
            self.seek(self.start, false);
        }
        let item = self.step_front();
        self.start += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining(), Some(self.remaining()))
    }
}

impl<'a, T> DoubleEndedIterator for Items<'a, T> {
    fn next_back(&mut self) -> Option<&'a T> {
        if self.start >= self.end {
            return None;
        }
        if self.back.is_empty() {
            self.seek(self.end - 1, true);
        }
        let item = self.step_back();
        self.end -= 1;
        Some(item)
    }
}

impl<T> ExactSizeIterator for Items<'_, T> {}

impl<T> std::iter::FusedIterator for Items<'_, T> {}

pub struct Owned<T> {
    items: std::vec::IntoIter<T>,
}

fn drain<T: Clone>(node: Arc<Node<T>>, out: &mut Vec<T>, depth: usize) {
    assert!(depth < HEIGHT_MAX);
    let (items, children) = match Arc::try_unwrap(node) {
        Ok(node) => (node.items, node.children),
        Err(shared) => (shared.items.clone(), shared.children.clone()),
    };
    if children.is_empty() {
        out.extend(items);
        return;
    }
    let mut children = children.into_iter();
    for item in items {
        let Some(child) = children.next() else {
            panic!("a branch has one more child than items")
        };
        drain(child.node, out, depth + 1);
        out.push(item);
    }
    let Some(last) = children.next() else {
        panic!("a branch has one more child than items")
    };
    drain(last.node, out, depth + 1);
}

impl<T: Clone> Owned<T> {
    pub(crate) fn new(tree: Tree<T>) -> Self {
        let mut items = Vec::with_capacity(tree.count);
        if let Some(root) = tree.root {
            drain(root, &mut items, 0);
        }
        assert!(items.len() == tree.count);
        Self {
            items: items.into_iter(),
        }
    }
}

impl<T> Iterator for Owned<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.items.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.items.size_hint()
    }
}

impl<T> DoubleEndedIterator for Owned<T> {
    fn next_back(&mut self) -> Option<T> {
        self.items.next_back()
    }
}

impl<T> ExactSizeIterator for Owned<T> {}

impl<T> std::iter::FusedIterator for Owned<T> {}
