use std::{
    borrow::Borrow,
    cmp::Ordering,
    ops::{Bound, RangeBounds},
    sync::Arc,
};

use nanocode_sys::InlineVec;

const NODE_ITEMS_MIN: usize = 5;
const NODE_ITEMS_MAX: usize = 16;
const NODE_ITEM_SLOTS: usize = NODE_ITEMS_MAX + 1;
const NODE_CHILD_SLOTS: usize = NODE_ITEM_SLOTS + 1;
const HEIGHT_MAX: usize = usize::BITS as usize;
const BRANCHLESS_SEARCH_ITEM_BYTES_MAX: usize = 16;
const SEARCH_STEPS: u32 = usize::BITS - NODE_ITEM_SLOTS.leading_zeros();

const _: () = assert!(2 * NODE_ITEMS_MIN <= NODE_ITEMS_MAX);
const _: () = assert!(NODE_CHILD_SLOTS <= u8::MAX as usize);

pub(crate) type Entries<T> = InlineVec<T, NODE_ITEM_SLOTS>;
type Children<T> = InlineVec<Child<T>, NODE_CHILD_SLOTS>;

pub(crate) struct Leaf<T> {
    entries: Entries<T>,
}

pub(crate) struct Branch<T> {
    entries: Entries<T>,
    children: Children<T>,
}

enum Link<T> {
    Leaf(Arc<Leaf<T>>),
    Branch(Arc<Branch<T>>),
}

struct Child<T> {
    count: usize,
    node: Link<T>,
}

impl<T> Clone for Link<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Leaf(leaf) => Self::Leaf(Arc::clone(leaf)),
            Self::Branch(branch) => Self::Branch(Arc::clone(branch)),
        }
    }
}

impl<T> Clone for Child<T> {
    fn clone(&self) -> Self {
        Self {
            count: self.count,
            node: self.node.clone(),
        }
    }
}

fn unique_leaf<T: Clone>(leaf: &mut Arc<Leaf<T>>) -> &mut Leaf<T> {
    if Arc::strong_count(leaf) > 1 {
        let shared = &**leaf;
        let copy = nanocode_sys::new_arc_with(
            || Leaf {
                entries: InlineVec::new(),
            },
            |fresh| fresh.entries.extend_cloned(&shared.entries),
        );
        *leaf = copy;
    }
    let Some(unique) = Arc::get_mut(leaf) else {
        panic!("a copied leaf has one owner")
    };
    unique
}

fn unique_branch<T: Clone>(branch: &mut Arc<Branch<T>>) -> &mut Branch<T> {
    if Arc::strong_count(branch) > 1 {
        let shared = &**branch;
        let copy = nanocode_sys::new_arc_with(
            || Branch {
                entries: InlineVec::new(),
                children: InlineVec::new(),
            },
            |fresh| {
                fresh.entries.extend_cloned(&shared.entries);
                fresh.children.extend_cloned(&shared.children);
            },
        );
        *branch = copy;
    }
    let Some(unique) = Arc::get_mut(branch) else {
        panic!("a copied branch has one owner")
    };
    unique
}

const fn is_plain<T>() -> bool {
    !std::mem::needs_drop::<T>() && size_of::<T>() <= BRANCHLESS_SEARCH_ITEM_BYTES_MAX
}

fn count_below<T>(entries: &[T], mut order: impl FnMut(&T) -> Ordering) -> usize {
    assert!(entries.len() <= NODE_ITEM_SLOTS);
    let mut below = 0;
    for shift in (0..SEARCH_STEPS).rev() {
        let probe = below + (1 << shift);
        let inside = probe <= entries.len();
        let less = inside && order(&entries[probe.min(entries.len()) - 1]).is_lt();
        below = std::hint::select_unpredictable(less, probe, below);
    }
    below
}

fn search<T>(entries: &[T], mut order: impl FnMut(&T) -> Ordering) -> Result<usize, usize> {
    if is_plain::<T>() {
        let below = count_below(entries, &mut order);
        return match entries.get(below).map(&mut order) {
            Some(Ordering::Equal) => Ok(below),
            _ => Err(below),
        };
    }
    let mut low = 0;
    let mut high = entries.len();
    for _ in 0..=usize::BITS {
        if low >= high {
            return Err(low);
        }
        let middle = low + (high - low) / 2;
        match order(&entries[middle]) {
            Ordering::Less => low = middle + 1,
            Ordering::Greater => high = middle,
            Ordering::Equal => return Ok(middle),
        }
    }
    panic!("a binary search halves its range each step")
}

const fn pair_mut<T>(items: &mut [T], left: usize) -> (&mut T, &mut T) {
    let (before, after) = items.split_at_mut(left + 1);
    (&mut before[left], &mut after[0])
}

pub(crate) enum Place {
    Found(usize),
    Below(usize),
}

pub(crate) trait Locate<T> {
    fn locate(&mut self, entries: &Entries<T>) -> Place;

    fn pick<'a>(&mut self, entries: &'a Entries<T>) -> Option<&'a T> {
        match self.locate(entries) {
            Place::Found(index) => Some(&entries[index]),
            Place::Below(_) => None,
        }
    }
}

pub(crate) trait Slot<T> {
    fn slot(&mut self, entries: &Entries<T>, item: &T) -> Place;
}

pub(crate) struct Path {
    steps: [u8; HEIGHT_MAX],
    len: usize,
    next: usize,
}

impl<T> Locate<T> for Path {
    fn locate(&mut self, entries: &Entries<T>) -> Place {
        assert!(self.next < self.len);
        let step = usize::from(self.steps[self.next]);
        self.next += 1;
        if self.next == self.len {
            assert!(step < entries.len());
            Place::Found(step)
        } else {
            assert!(step <= entries.len());
            Place::Below(step)
        }
    }
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
    fn locate(&mut self, entries: &Entries<T>) -> Place {
        match search(entries, |item| (self.project)(item).borrow().cmp(self.key)) {
            Ok(index) => Place::Found(index),
            Err(index) => Place::Below(index),
        }
    }

    fn pick<'a>(&mut self, entries: &'a Entries<T>) -> Option<&'a T> {
        if !is_plain::<T>() {
            return search(entries, |item| (self.project)(item).borrow().cmp(self.key))
                .ok()
                .map(|index| &entries[index]);
        }
        let last = entries.len().checked_sub(1)?;
        let below = count_below(entries, |item| (self.project)(item).borrow().cmp(self.key));
        let item = &entries[below.min(last)];
        let found = (self.project)(item).borrow().cmp(self.key).is_eq();
        std::hint::select_unpredictable(found, Some(item), None)
    }
}

pub(crate) struct ByOrder<F>(pub(crate) F);

impl<T, K, F> Slot<T> for ByOrder<F>
where
    K: Ord,
    F: Fn(&T) -> &K,
{
    fn slot(&mut self, entries: &Entries<T>, item: &T) -> Place {
        let key = (self.0)(item);
        match search(entries, |existing| (self.0)(existing).cmp(key)) {
            Ok(index) => Place::Found(index),
            Err(index) => Place::Below(index),
        }
    }
}

enum Grown<T> {
    Replaced(T),
    Added,
    Split(T, Link<T>),
}

type Parts<'a, T> = (&'a mut Entries<T>, Option<&'a mut Children<T>>);

impl<T> Link<T> {
    fn entries(&self) -> &Entries<T> {
        match self {
            Self::Leaf(leaf) => &leaf.entries,
            Self::Branch(branch) => &branch.entries,
        }
    }

    fn children(&self) -> Option<&Children<T>> {
        match self {
            Self::Leaf(_) => None,
            Self::Branch(branch) => Some(&branch.children),
        }
    }

    fn count(&self) -> usize {
        self.entries().len()
            + self
                .children()
                .map_or(0, |children| children.iter().map(|child| child.count).sum())
    }

    fn assemble(entries: Entries<T>, children: Option<Children<T>>) -> Self {
        match children {
            None => Self::Leaf(Arc::new(Leaf { entries })),
            Some(children) => Self::Branch(Arc::new(Branch { entries, children })),
        }
    }
}

impl<T> Branch<T> {
    const fn frame(&self, index: usize) -> Frame<'_, T> {
        Frame {
            entries: &self.entries,
            children: &self.children,
            index,
        }
    }
}

fn child<T>(children: &Children<T>, index: usize) -> &Link<T> {
    &children[index].node
}

impl<T: Clone> Link<T> {
    fn parts_mut(&mut self) -> Parts<'_, T> {
        match self {
            Self::Leaf(leaf) => (&mut unique_leaf(leaf).entries, None),
            Self::Branch(branch) => {
                let branch = unique_branch(branch);
                (&mut branch.entries, Some(&mut branch.children))
            }
        }
    }

    fn insert(&mut self, item: T, slot: &mut impl Slot<T>, rightmost: bool) -> Grown<T> {
        let (entries, children) = self.parts_mut();
        let index = match slot.slot(entries, &item) {
            Place::Found(index) => {
                return Grown::Replaced(std::mem::replace(&mut entries[index], item));
            }
            Place::Below(index) => index,
        };
        let children = match children {
            None => {
                entries.insert(index, item);
                None
            }
            Some(children) => {
                let last = index + 1 == children.len();
                let child = &mut children[index];
                match child.node.insert(item, slot, rightmost && last) {
                    Grown::Replaced(old) => return Grown::Replaced(old),
                    Grown::Added => {
                        child.count += 1;
                        return Grown::Added;
                    }
                    Grown::Split(median, right) => {
                        child.count += 1;
                        adopt(entries, children, index, median, right);
                        Some(children)
                    }
                }
            }
        };
        if entries.len() > NODE_ITEMS_MAX {
            let at_end = index + 1 == entries.len();
            let (median, right) = split(entries, children, rightmost && at_end);
            return Grown::Split(median, right);
        }
        Grown::Added
    }

    fn push_last(&mut self, item: T) -> Option<(T, Self)> {
        let (entries, children) = self.parts_mut();
        let children = match children {
            None => {
                entries.push(item);
                None
            }
            Some(children) => {
                let index = children.len() - 1;
                let child = &mut children[index];
                let grown = child.node.push_last(item);
                child.count += 1;
                if let Some((median, right)) = grown {
                    adopt(entries, children, index, median, right);
                }
                Some(children)
            }
        };
        if entries.len() > NODE_ITEMS_MAX {
            return Some(split(entries, children, true));
        }
        None
    }

    fn remove(&mut self, locate: &mut impl Locate<T>) -> T {
        let (entries, children) = self.parts_mut();
        match (locate.locate(entries), children) {
            (Place::Found(index), None) => entries.remove(index),
            (Place::Found(index), Some(children)) => {
                let child = &mut children[index];
                let predecessor = child.node.remove_last();
                child.count -= 1;
                let old = std::mem::replace(&mut entries[index], predecessor);
                rebalance(entries, children, index);
                old
            }
            (Place::Below(_), None) => panic!("a removed item is in the tree"),
            (Place::Below(index), Some(children)) => {
                let child = &mut children[index];
                let old = child.node.remove(locate);
                child.count -= 1;
                rebalance(entries, children, index);
                old
            }
        }
    }

    fn remove_first(&mut self) -> T {
        let (entries, children) = self.parts_mut();
        let Some(children) = children else {
            assert!(!entries.is_empty());
            return entries.remove(0);
        };
        let child = &mut children[0];
        let first = child.node.remove_first();
        child.count -= 1;
        rebalance(entries, children, 0);
        first
    }

    fn remove_last(&mut self) -> T {
        let (entries, children) = self.parts_mut();
        let Some(children) = children else {
            let Some(last) = entries.pop() else {
                panic!("a non-root node holds items")
            };
            return last;
        };
        let index = children.len() - 1;
        let child = &mut children[index];
        let last = child.node.remove_last();
        child.count -= 1;
        rebalance(entries, children, index);
        last
    }
}

fn split<T>(
    entries: &mut Entries<T>,
    children: Option<&mut Children<T>>,
    rightmost: bool,
) -> (T, Link<T>) {
    assert!(entries.len() > NODE_ITEMS_MAX);
    let middle = if rightmost {
        entries.len() - 2
    } else {
        entries.len() / 2
    };
    let right = entries.split_off(middle + 1);
    let Some(median) = entries.pop() else {
        panic!("a full node has a median")
    };
    let right_children = children.map(|children| children.split_off(middle + 1));
    (median, Link::assemble(right, right_children))
}

fn adopt<T>(
    entries: &mut Entries<T>,
    children: &mut Children<T>,
    index: usize,
    median: T,
    right: Link<T>,
) {
    let right_count = right.count();
    children[index].count -= right_count + 1;
    entries.insert(index, median);
    children.insert(
        index + 1,
        Child {
            count: right_count,
            node: right,
        },
    );
}

fn held<T>(children: &Children<T>, index: usize) -> usize {
    child(children, index).entries().len()
}

fn rebalance<T: Clone>(entries: &mut Entries<T>, children: &mut Children<T>, index: usize) {
    let held_here = held(children, index);
    if held_here >= NODE_ITEMS_MIN {
        return;
    }
    let left = index.checked_sub(1).map(|left| held(children, left));
    let right = (index + 1 < children.len()).then(|| held(children, index + 1));
    match (left, right) {
        (Some(left), _) if left + 1 + held_here <= NODE_ITEMS_MAX => {
            merge(entries, children, index - 1);
        }
        (_, Some(right)) if held_here + 1 + right <= NODE_ITEMS_MAX => {
            merge(entries, children, index);
        }
        (Some(left), _) if left > NODE_ITEMS_MIN => {
            shift_right(entries, children, index, lent(left, held_here));
        }
        (_, Some(right)) if right > NODE_ITEMS_MIN => {
            shift_left(entries, children, index, lent(right, held_here));
        }
        _ => panic!("a short node has a sibling to merge with or borrow from"),
    }
}

fn lent(lender: usize, held: usize) -> usize {
    assert!(lender > NODE_ITEMS_MIN);
    assert!(held < NODE_ITEMS_MIN);
    ((lender - held) / 2).min(lender - NODE_ITEMS_MIN)
}

fn shift_right<T: Clone>(
    entries: &mut Entries<T>,
    children: &mut Children<T>,
    index: usize,
    lent: usize,
) {
    assert!(index > 0);
    let separator = &mut entries[index - 1];
    let (left, right) = pair_mut(children, index - 1);
    let (left_entries, mut left_children) = left.node.parts_mut();
    let (right_entries, mut right_children) = right.node.parts_mut();
    assert!(left_children.is_some() == right_children.is_some());
    let mut moved = 0;
    for _ in 0..lent {
        let Some(borrowed) = left_entries.pop() else {
            panic!("a lending sibling holds items")
        };
        let lowered = std::mem::replace(separator, borrowed);
        right_entries.insert(0, lowered);
        moved += 1;
        if let (Some(from), Some(to)) =
            (left_children.as_deref_mut(), right_children.as_deref_mut())
        {
            let Some(child) = from.pop() else {
                panic!("a lending branch holds children")
            };
            moved += child.count;
            to.insert(0, child);
        }
    }
    left.count -= moved;
    right.count += moved;
}

fn shift_left<T: Clone>(
    entries: &mut Entries<T>,
    children: &mut Children<T>,
    index: usize,
    lent: usize,
) {
    let separator = &mut entries[index];
    let (left, right) = pair_mut(children, index);
    let (left_entries, mut left_children) = left.node.parts_mut();
    let (right_entries, mut right_children) = right.node.parts_mut();
    assert!(left_children.is_some() == right_children.is_some());
    let mut moved = 0;
    for _ in 0..lent {
        let borrowed = right_entries.remove(0);
        let lowered = std::mem::replace(separator, borrowed);
        left_entries.push(lowered);
        moved += 1;
        if let (Some(to), Some(from)) =
            (left_children.as_deref_mut(), right_children.as_deref_mut())
        {
            let child = from.remove(0);
            moved += child.count;
            to.push(child);
        }
    }
    left.count += moved;
    right.count -= moved;
}

fn merge<T: Clone>(entries: &mut Entries<T>, children: &mut Children<T>, index: usize) {
    let separator = entries.remove(index);
    let right = children.remove(index + 1);
    let left = &mut children[index];
    let (left_entries, left_children) = left.node.parts_mut();
    left_entries.push(separator);
    match (right.node, left_children) {
        (Link::Leaf(leaf), None) => match Arc::try_unwrap(leaf) {
            Ok(leaf) => left_entries.append(leaf.entries),
            Err(shared) => left_entries.extend_cloned(&shared.entries),
        },
        (Link::Branch(branch), Some(left_children)) => match Arc::try_unwrap(branch) {
            Ok(branch) => {
                left_entries.append(branch.entries);
                left_children.append(branch.children);
            }
            Err(shared) => {
                left_entries.extend_cloned(&shared.entries);
                left_children.extend_cloned(&shared.children);
            }
        },
        _ => panic!("siblings sit at one height"),
    }
    assert!(left_entries.len() <= NODE_ITEMS_MAX);
    left.count += 1 + right.count;
}

pub(crate) struct Tree<T> {
    root: Option<Link<T>>,
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

fn below<T, K, Q>(
    entries: &Entries<T>,
    key: &Q,
    inclusive: bool,
    project: &impl Fn(&T) -> &K,
) -> usize
where
    K: Borrow<Q>,
    Q: Ord + ?Sized,
{
    entries.partition_point(|item| match project(item).borrow().cmp(key) {
        Ordering::Less => true,
        Ordering::Equal => inclusive,
        Ordering::Greater => false,
    })
}

fn counted<T>(children: &Children<T>, below: usize) -> usize {
    children
        .iter()
        .take(below)
        .map(|child| child.count)
        .sum::<usize>()
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
        let mut link = self.root.as_ref()?;
        for _ in 0..HEIGHT_MAX {
            match link {
                Link::Leaf(leaf) => return locate.pick(&leaf.entries),
                Link::Branch(branch) => match locate.locate(&branch.entries) {
                    Place::Found(index) => return Some(&branch.entries[index]),
                    Place::Below(index) => link = child(&branch.children, index),
                },
            }
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    pub(crate) fn path(&self, mut locate: impl Locate<T>) -> Option<Path> {
        let mut path = Path {
            steps: [0; HEIGHT_MAX],
            len: 0,
            next: 0,
        };
        let mut link = self.root.as_ref()?;
        for _ in 0..HEIGHT_MAX {
            let place = locate.locate(link.entries());
            let (Place::Found(step) | Place::Below(step)) = place;
            let Ok(step) = u8::try_from(step) else {
                panic!("a node holds fewer than {} children", u8::MAX)
            };
            path.steps[path.len] = step;
            path.len += 1;
            match (place, link) {
                (Place::Found(_), _) => return Some(path),
                (Place::Below(_), Link::Leaf(_)) => return None,
                (Place::Below(index), Link::Branch(branch)) => {
                    link = child(&branch.children, index);
                }
            }
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    pub(crate) fn first(&self) -> Option<&T> {
        let mut link = self.root.as_ref()?;
        for _ in 0..HEIGHT_MAX {
            match link {
                Link::Leaf(leaf) => return Some(&leaf.entries[0]),
                Link::Branch(branch) => link = child(&branch.children, 0),
            }
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    pub(crate) fn last(&self) -> Option<&T> {
        let mut link = self.root.as_ref()?;
        for _ in 0..HEIGHT_MAX {
            match link {
                Link::Leaf(leaf) => return Some(&leaf.entries[leaf.entries.len() - 1]),
                Link::Branch(branch) => {
                    link = child(&branch.children, branch.children.len() - 1);
                }
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
        let Some(mut link) = self.root.as_ref() else {
            return 0;
        };
        for _ in 0..HEIGHT_MAX {
            match link {
                Link::Leaf(leaf) => return rank + below(&leaf.entries, key, inclusive, project),
                Link::Branch(branch) => {
                    let below = below(&branch.entries, key, inclusive, project);
                    rank += below + counted(&branch.children, below);
                    link = child(&branch.children, below);
                }
            }
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    fn seek_key<'a, K, Q>(
        &'a self,
        key: &Q,
        inclusive: bool,
        project: &impl Fn(&T) -> &K,
        items: &mut Items<'a, T>,
    ) -> usize
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let mut rank = 0;
        let Some(mut link) = self.root.as_ref() else {
            return 0;
        };
        for _ in 0..HEIGHT_MAX {
            match link {
                Link::Leaf(leaf) => {
                    let below = below(&leaf.entries, key, inclusive, project);
                    items.front_leaf = &leaf.entries[below..];
                    return rank + below;
                }
                Link::Branch(branch) => {
                    let below = below(&branch.entries, key, inclusive, project);
                    rank += below + counted(&branch.children, below);
                    items.front.push(branch.frame(below));
                    link = child(&branch.children, below);
                }
            }
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
        let mut items = self.items(0, self.count);
        let start = match range.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(key) => self.seek_key(key, false, &project, &mut items),
            Bound::Excluded(key) => self.seek_key(key, true, &project, &mut items),
        };
        let end = match range.end_bound() {
            Bound::Unbounded => self.count,
            Bound::Included(key) => self.rank(key, true, &project),
            Bound::Excluded(key) => self.rank(key, false, &project),
        };
        items.start = start;
        items.end = end.max(start);
        items
    }

    pub(crate) fn items(&self, start: usize, end: usize) -> Items<'_, T> {
        assert!(start <= end);
        assert!(end <= self.count);
        Items {
            root: self.root.as_ref(),
            front_leaf: &[],
            front: Vec::new(),
            back_leaf: &[],
            back: Vec::new(),
            start,
            end,
        }
    }
}

impl<T: Clone> Tree<T> {
    pub(crate) fn find_mut(&mut self, mut locate: impl Locate<T>) -> Option<&mut T> {
        let mut link = self.root.as_mut()?;
        for _ in 0..HEIGHT_MAX {
            let (entries, children) = link.parts_mut();
            match (locate.locate(entries), children) {
                (Place::Found(index), _) => return Some(&mut entries[index]),
                (Place::Below(_), None) => return None,
                (Place::Below(index), Some(children)) => link = &mut children[index].node,
            }
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    fn grow(&mut self, median: T, right: Link<T>) {
        let Some(root) = self.root.take() else {
            panic!("a split needs a root")
        };
        let right_count = right.count();
        let left_count = self.count - right_count - 1;
        let mut entries = InlineVec::new();
        entries.push(median);
        let mut children = InlineVec::new();
        children.push(Child {
            count: left_count,
            node: root,
        });
        children.push(Child {
            count: right_count,
            node: right,
        });
        self.root = Some(Link::Branch(Arc::new(Branch { entries, children })));
    }

    fn single(item: T) -> Link<T> {
        let mut entries = InlineVec::new();
        entries.push(item);
        Link::Leaf(Arc::new(Leaf { entries }))
    }

    pub(crate) fn insert(&mut self, item: T, mut slot: impl Slot<T>) -> Option<T> {
        let Some(root) = self.root.as_mut() else {
            assert!(self.count == 0);
            self.root = Some(Self::single(item));
            self.count = 1;
            return None;
        };
        match root.insert(item, &mut slot, true) {
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
            self.root = Some(Self::single(item));
            self.count = 1;
            return;
        };
        let grown = root.push_last(item);
        self.count += 1;
        if let Some((median, right)) = grown {
            self.grow(median, right);
        }
    }

    fn shrink(&mut self, removed: T) -> T {
        let Some(root) = self.root.as_mut() else {
            panic!("a removal had a root")
        };
        let (entries, children) = root.parts_mut();
        if entries.is_empty() {
            self.root = children.and_then(InlineVec::pop).map(|child| child.node);
        }
        self.count -= 1;
        assert!(self.count > 0 || self.root.is_none());
        removed
    }

    pub(crate) fn remove(&mut self, mut locate: impl Locate<T>) -> T {
        let Some(root) = self.root.as_mut() else {
            panic!("removal from an empty tree")
        };
        let removed = root.remove(&mut locate);
        self.shrink(removed)
    }

    pub(crate) fn pop_first(&mut self) -> Option<T> {
        let removed = self.root.as_mut()?.remove_first();
        Some(self.shrink(removed))
    }

    pub(crate) fn pop_last(&mut self) -> Option<T> {
        let removed = self.root.as_mut()?.remove_last();
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

struct Frame<'a, T> {
    entries: &'a Entries<T>,
    children: &'a Children<T>,
    index: usize,
}

impl<T> Clone for Frame<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Frame<'_, T> {}

pub struct Items<'a, T> {
    root: Option<&'a Link<T>>,
    front_leaf: &'a [T],
    front: Vec<Frame<'a, T>>,
    back_leaf: &'a [T],
    back: Vec<Frame<'a, T>>,
    start: usize,
    end: usize,
}

impl<T> Clone for Items<'_, T> {
    fn clone(&self) -> Self {
        Self {
            root: self.root,
            front_leaf: self.front_leaf,
            front: self.front.clone(),
            back_leaf: self.back_leaf,
            back: self.back.clone(),
            start: self.start,
            end: self.end,
        }
    }
}

impl<'a, T> Items<'a, T> {
    fn root(&self) -> &'a Link<T> {
        let Some(root) = self.root else {
            panic!("a non-empty range has a root")
        };
        root
    }

    fn seek_front(&mut self, position: usize) {
        let mut link = self.root();
        let mut position = position;
        'descend: for _ in 0..HEIGHT_MAX {
            let branch = match link {
                Link::Leaf(leaf) => {
                    self.front_leaf = &leaf.entries[position..];
                    return;
                }
                Link::Branch(branch) => branch,
            };
            for (index, child) in branch.children.iter().enumerate() {
                match position.cmp(&child.count) {
                    Ordering::Less => {
                        self.front.push(branch.frame(index));
                        link = &child.node;
                        continue 'descend;
                    }
                    Ordering::Equal => {
                        self.front.push(branch.frame(index));
                        return;
                    }
                    Ordering::Greater => position -= child.count + 1,
                }
            }
            panic!("position beyond the subtree's count")
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    fn seek_back(&mut self, position: usize) {
        let mut link = self.root();
        let mut position = position;
        'descend: for _ in 0..HEIGHT_MAX {
            let branch = match link {
                Link::Leaf(leaf) => {
                    self.back_leaf = &leaf.entries[..=position];
                    return;
                }
                Link::Branch(branch) => branch,
            };
            for (index, child) in branch.children.iter().enumerate() {
                match position.cmp(&child.count) {
                    Ordering::Less => {
                        self.back.push(branch.frame(index));
                        link = &child.node;
                        continue 'descend;
                    }
                    Ordering::Equal => {
                        self.back.push(branch.frame(index + 1));
                        return;
                    }
                    Ordering::Greater => position -= child.count + 1,
                }
            }
            panic!("position beyond the subtree's count")
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    fn descend_front(&mut self, mut link: &'a Link<T>) {
        for _ in 0..HEIGHT_MAX {
            match link {
                Link::Leaf(leaf) => {
                    self.front_leaf = &leaf.entries;
                    return;
                }
                Link::Branch(branch) => {
                    self.front.push(branch.frame(0));
                    link = child(&branch.children, 0);
                }
            }
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    fn descend_back(&mut self, mut link: &'a Link<T>) {
        for _ in 0..HEIGHT_MAX {
            match link {
                Link::Leaf(leaf) => {
                    self.back_leaf = &leaf.entries;
                    return;
                }
                Link::Branch(branch) => {
                    let last = branch.children.len() - 1;
                    self.back.push(branch.frame(last));
                    link = child(&branch.children, last);
                }
            }
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    fn step_front(&mut self) -> &'a T {
        for _ in 0..HEIGHT_MAX {
            let Some(top) = self.front.last_mut() else {
                panic!("a front cursor inside the range")
            };
            let frame = *top;
            if frame.index < frame.entries.len() {
                top.index += 1;
                self.descend_front(child(frame.children, frame.index + 1));
                return &frame.entries[frame.index];
            }
            self.front.pop();
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
    }

    fn step_back(&mut self) -> &'a T {
        for _ in 0..HEIGHT_MAX {
            let Some(top) = self.back.last_mut() else {
                panic!("a back cursor inside the range")
            };
            let frame = *top;
            if frame.index > 0 {
                top.index -= 1;
                self.descend_back(child(frame.children, frame.index - 1));
                return &frame.entries[frame.index - 1];
            }
            self.back.pop();
        }
        panic!("tree deeper than {HEIGHT_MAX} levels")
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
        self.start += 1;
        if let Some((item, rest)) = self.front_leaf.split_first() {
            self.front_leaf = rest;
            return Some(item);
        }
        if self.front.is_empty() {
            self.seek_front(self.start - 1);
            if let Some((item, rest)) = self.front_leaf.split_first() {
                self.front_leaf = rest;
                return Some(item);
            }
        }
        Some(self.step_front())
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
        self.end -= 1;
        if let Some((item, rest)) = self.back_leaf.split_last() {
            self.back_leaf = rest;
            return Some(item);
        }
        if self.back.is_empty() {
            self.seek_back(self.end);
            if let Some((item, rest)) = self.back_leaf.split_last() {
                self.back_leaf = rest;
                return Some(item);
            }
        }
        Some(self.step_back())
    }
}

impl<T> ExactSizeIterator for Items<'_, T> {}

impl<T> std::iter::FusedIterator for Items<'_, T> {}

pub struct Owned<T> {
    items: std::vec::IntoIter<T>,
}

fn drain<T: Clone>(link: Link<T>, out: &mut Vec<T>, depth: usize) {
    assert!(depth < HEIGHT_MAX);
    let (entries, children) = match link {
        Link::Leaf(leaf) => {
            let entries = Arc::try_unwrap(leaf)
                .map_or_else(|shared| shared.entries.clone(), |leaf| leaf.entries);
            out.extend(entries);
            return;
        }
        Link::Branch(branch) => match Arc::try_unwrap(branch) {
            Ok(branch) => (branch.entries, branch.children),
            Err(shared) => (shared.entries.clone(), shared.children.clone()),
        },
    };
    let mut children = children.into_iter();
    for item in entries {
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
        Self::from_items(items)
    }
}

impl<T> Owned<T> {
    pub(crate) fn from_items(items: Vec<T>) -> Self {
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
