use std::sync::Arc;

pub(crate) const VECTOR_LEAF_ITEMS: usize = 16;
const VECTOR_BRANCH_CHILDREN: usize = 32;
pub(crate) const LEAF_BITS: u32 = VECTOR_LEAF_ITEMS.trailing_zeros();
pub(crate) const LEAF_MASK: usize = VECTOR_LEAF_ITEMS - 1;
const BRANCH_BITS: u32 = VECTOR_BRANCH_CHILDREN.trailing_zeros();
const BRANCH_MASK: usize = VECTOR_BRANCH_CHILDREN - 1;
const SPINE_HEIGHT_MAX: usize = ((usize::BITS - LEAF_BITS) / BRANCH_BITS) as usize;

const _: () = assert!(VECTOR_LEAF_ITEMS.is_power_of_two());
const _: () = assert!(VECTOR_BRANCH_CHILDREN.is_power_of_two());

pub(crate) struct Leaf<A> {
    pub(crate) slots: [Option<A>; VECTOR_LEAF_ITEMS],
}

impl<A> Leaf<A> {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [const { None }; VECTOR_LEAF_ITEMS],
        }
    }

    pub(crate) const fn item(&self, index: usize) -> &A {
        let Some(item) = &self.slots[index] else {
            panic!("a live slot holds an item")
        };
        item
    }

    pub(crate) const fn item_mut(&mut self, index: usize) -> &mut A {
        let Some(item) = &mut self.slots[index] else {
            panic!("a live slot holds an item")
        };
        item
    }

    pub(crate) fn take(&mut self, index: usize) -> A {
        let Some(item) = self.slots[index].take() else {
            panic!("a live slot holds an item")
        };
        item
    }
}

pub(crate) fn unique_leaf<A: Clone>(
    leaf: &mut Arc<Leaf<A>>,
    start: usize,
    end: usize,
) -> &mut Leaf<A> {
    if Arc::strong_count(leaf) > 1 {
        let shared = &**leaf;
        let copy = nanocode_sys::new_arc_with(Leaf::new, |fresh| {
            for (slot, item) in fresh.slots[start..end]
                .iter_mut()
                .zip(&shared.slots[start..end])
            {
                *slot = item.clone();
            }
        });
        *leaf = copy;
    }
    let Some(unique) = Arc::get_mut(leaf) else {
        panic!("a copied leaf has one owner")
    };
    unique
}

fn unique_branch<A>(branch: &mut Arc<Branch<A>>) -> &mut Branch<A> {
    if Arc::strong_count(branch) > 1 {
        let shared = &**branch;
        let copy = nanocode_sys::new_arc_with(Branch::new, |fresh| {
            fresh.children.clone_from(&shared.children);
        });
        *branch = copy;
    }
    let Some(unique) = Arc::get_mut(branch) else {
        panic!("a copied branch has one owner")
    };
    unique
}

enum Link<A> {
    Leaf(Arc<Leaf<A>>),
    Branch(Arc<Branch<A>>),
}

impl<A> Clone for Link<A> {
    fn clone(&self) -> Self {
        match self {
            Self::Leaf(leaf) => Self::Leaf(Arc::clone(leaf)),
            Self::Branch(branch) => Self::Branch(Arc::clone(branch)),
        }
    }
}

struct Branch<A> {
    children: [Option<Link<A>>; VECTOR_BRANCH_CHILDREN],
}

impl<A> Branch<A> {
    const fn new() -> Self {
        Self {
            children: [const { None }; VECTOR_BRANCH_CHILDREN],
        }
    }
}

const fn child_index(position: usize, level: usize) -> usize {
    (position >> (BRANCH_BITS as usize * level)) & BRANCH_MASK
}

const fn span(height: usize) -> usize {
    assert!(height <= SPINE_HEIGHT_MAX);
    1 << (BRANCH_BITS as usize * height)
}

pub(crate) struct Spine<A> {
    root: Option<Link<A>>,
    height: usize,
    start: usize,
    count: usize,
}

impl<A> Clone for Spine<A> {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            height: self.height,
            start: self.start,
            count: self.count,
        }
    }
}

impl<A> Spine<A> {
    pub(crate) const fn new() -> Self {
        Self {
            root: None,
            height: 0,
            start: 0,
            count: 0,
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.count
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub(crate) fn leaf(&self, index: usize) -> &Leaf<A> {
        assert!(index < self.count);
        let position = self.start + index;
        let Some(mut link) = self.root.as_ref() else {
            panic!("a non-empty spine has a root")
        };
        for level in (0..self.height).rev() {
            let Link::Branch(branch) = link else {
                panic!("a link above the leaves is a branch")
            };
            let Some(child) = &branch.children[child_index(position, level)] else {
                panic!("a live position has a child")
            };
            link = child;
        }
        let Link::Leaf(leaf) = link else {
            panic!("a link at the bottom is a leaf")
        };
        leaf
    }
}

impl<A: Clone> Spine<A> {
    pub(crate) fn leaf_mut(&mut self, index: usize) -> &mut Leaf<A> {
        assert!(index < self.count);
        let position = self.start + index;
        let Some(mut link) = self.root.as_mut() else {
            panic!("a non-empty spine has a root")
        };
        for level in (0..self.height).rev() {
            let Link::Branch(branch) = link else {
                panic!("a link above the leaves is a branch")
            };
            let Some(child) = &mut unique_branch(branch).children[child_index(position, level)]
            else {
                panic!("a live position has a child")
            };
            link = child;
        }
        let Link::Leaf(leaf) = link else {
            panic!("a link at the bottom is a leaf")
        };
        unique_leaf(leaf, 0, VECTOR_LEAF_ITEMS)
    }

    pub(crate) fn push_last(&mut self, leaf: Arc<Leaf<A>>) {
        let Some(root) = self.root.take() else {
            assert!(self.count == 0);
            self.root = Some(Link::Leaf(leaf));
            self.height = 0;
            self.start = 0;
            self.count = 1;
            return;
        };
        let position = self.start + self.count;
        let root = if position == span(self.height) {
            let mut branch = Branch::new();
            branch.children[0] = Some(root);
            self.height += 1;
            assert!(self.height <= SPINE_HEIGHT_MAX);
            Link::Branch(Arc::new(branch))
        } else {
            root
        };
        let root = self.root.insert(root);
        let mut link = root;
        for level in (1..self.height).rev() {
            let Link::Branch(branch) = link else {
                panic!("a link above the leaves is a branch")
            };
            let slot = &mut unique_branch(branch).children[child_index(position, level)];
            link = slot.get_or_insert_with(|| Link::Branch(Arc::new(Branch::new())));
        }
        let Link::Branch(branch) = link else {
            panic!("a link above the leaves is a branch")
        };
        let slot = &mut unique_branch(branch).children[child_index(position, 0)];
        assert!(slot.is_none());
        *slot = Some(Link::Leaf(leaf));
        self.count += 1;
    }

    fn take(
        link: &mut Link<A>,
        level: usize,
        position: usize,
        live: (usize, usize),
    ) -> Arc<Leaf<A>> {
        let Link::Branch(branch) = link else {
            panic!("a link above the leaves is a branch")
        };
        let slot = &mut unique_branch(branch).children[child_index(position, level)];
        if level == 0 {
            let Some(Link::Leaf(leaf)) = slot.take() else {
                panic!("a live position holds a leaf")
            };
            return leaf;
        }
        let Some(child) = slot.as_mut() else {
            panic!("a live position has a child")
        };
        let leaf = Self::take(child, level - 1, position, live);
        let shift = BRANCH_BITS as usize * level;
        let first = position >> shift << shift;
        let after = first + (1 << shift);
        if live.1 <= first || after <= live.0 {
            *slot = None;
        }
        leaf
    }

    fn remove(&mut self, position: usize, live: (usize, usize)) -> Arc<Leaf<A>> {
        assert!(self.count > 0);
        if self.height == 0 {
            assert!(self.count == 1);
            let Some(Link::Leaf(leaf)) = self.root.take() else {
                panic!("a spine of height zero is one leaf")
            };
            return leaf;
        }
        let Some(root) = self.root.as_mut() else {
            panic!("a non-empty spine has a root")
        };
        Self::take(root, self.height - 1, position, live)
    }

    fn settle(&mut self) {
        if self.count == 0 {
            self.root = None;
            self.height = 0;
            self.start = 0;
            return;
        }
        for _ in 0..=SPINE_HEIGHT_MAX {
            if self.height == 0 {
                return;
            }
            let child_span = span(self.height - 1);
            let Some(Link::Branch(root)) = self.root.as_mut() else {
                panic!("a spine above height zero has a branch root")
            };
            let consumed = self.start / child_span;
            if consumed > 0 {
                let branch = unique_branch(root);
                assert!(branch.children[..consumed].iter().all(Option::is_none));
                branch.children.rotate_left(consumed);
                self.start -= consumed * child_span;
            }
            if self.start + self.count > child_span {
                return;
            }
            let Some(first) = root.children[0].clone() else {
                panic!("the first child holds the live leaves")
            };
            self.root = Some(first);
            self.height -= 1;
        }
        panic!("spine deeper than {SPINE_HEIGHT_MAX} levels")
    }

    pub(crate) fn pop_first(&mut self) -> Arc<Leaf<A>> {
        let end = self.start + self.count;
        let leaf = self.remove(self.start, (self.start + 1, end));
        self.start += 1;
        self.count -= 1;
        self.settle();
        leaf
    }

    pub(crate) fn pop_last(&mut self) -> Arc<Leaf<A>> {
        let last = self.start + self.count - 1;
        let leaf = self.remove(last, (self.start, last));
        self.count -= 1;
        self.settle();
        leaf
    }
}
