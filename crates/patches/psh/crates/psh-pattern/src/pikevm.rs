use alloc::vec;
use alloc::vec::Vec;

use crate::look;
use crate::nfa::{Nfa, State, StateId};

pub const NONE: usize = usize::MAX;

#[derive(Clone, Debug)]
struct SparseSet {
    dense: Vec<StateId>,
    sparse: Vec<u32>,
}

impl SparseSet {
    fn new(capacity: usize) -> SparseSet {
        SparseSet {
            dense: Vec::with_capacity(capacity),
            sparse: vec![0; capacity],
        }
    }

    fn resize(&mut self, capacity: usize) {
        self.dense.clear();
        self.dense.reserve(capacity);
        self.sparse.clear();
        self.sparse.resize(capacity, 0);
    }

    fn contains(&self, id: StateId) -> bool {
        let index = self.sparse[id as usize] as usize;
        index < self.dense.len() && self.dense[index] == id
    }

    fn insert(&mut self, id: StateId) -> bool {
        if self.contains(id) {
            return false;
        }
        self.sparse[id as usize] = self.dense.len() as u32;
        self.dense.push(id);
        true
    }

    fn clear(&mut self) {
        self.dense.clear();
    }
}

#[derive(Clone, Debug)]
struct Active {
    set: SparseSet,
    slots: Vec<usize>,
    stride: usize,
}

impl Active {
    fn new() -> Active {
        Active {
            set: SparseSet::new(0),
            slots: Vec::new(),
            stride: 0,
        }
    }

    fn setup(&mut self, states: usize, stride: usize) {
        self.set.resize(states);
        self.stride = stride;
        self.slots.clear();
        self.slots.resize(states * stride, NONE);
    }

    fn for_state(&mut self, id: StateId) -> &mut [usize] {
        let start = id as usize * self.stride;
        &mut self.slots[start..start + self.stride]
    }
}

#[derive(Clone, Copy, Debug)]
enum Frame {
    Explore(StateId),
    Restore { slot: usize, offset: usize },
}

#[derive(Clone, Debug)]
pub struct PikeCache {
    stack: Vec<Frame>,
    current: Active,
    next: Active,
    scratch: Vec<usize>,
}

impl PikeCache {
    pub fn new() -> PikeCache {
        PikeCache {
            stack: Vec::new(),
            current: Active::new(),
            next: Active::new(),
            scratch: Vec::new(),
        }
    }
}

impl Default for PikeCache {
    fn default() -> PikeCache {
        PikeCache::new()
    }
}

pub struct Search<'h> {
    pub haystack: &'h [u8],
    pub start: usize,
    pub end: usize,
    pub anchored: bool,
    pub earliest: bool,
}

pub fn search(
    nfa: &Nfa,
    cache: &mut PikeCache,
    search: &Search<'_>,
    line_terminator: u8,
    slots: &mut [usize],
) -> Option<usize> {
    assert!(!nfa.reverse);
    let stride = slots.len();
    cache.current.setup(nfa.states.len(), stride);
    cache.next.setup(nfa.states.len(), stride);
    cache.scratch.clear();
    cache.scratch.resize(stride, NONE);
    if search.start > search.end {
        return None;
    }
    let anchored = search.anchored || nfa.always_anchored;
    let start_id = nfa.start_anchored;
    let PikeCache {
        stack,
        current,
        next,
        scratch,
    } = cache;
    let mut matched = None;
    let mut at = search.start;
    while at <= search.end {
        if current.set.dense.is_empty() {
            if matched.is_some() {
                break;
            }
            if anchored && at > search.start {
                break;
            }
        }
        if matched.is_none() && (!anchored || at == search.start) {
            scratch.iter_mut().for_each(|s| *s = NONE);
            epsilon_closure(
                nfa,
                stack,
                scratch,
                current,
                search.haystack,
                at,
                start_id,
                line_terminator,
            );
        }
        let mut found = false;
        for index in 0..current.set.dense.len() {
            let id = current.set.dense[index];
            match &nfa.states[id as usize] {
                State::Match => {
                    let source = id as usize * current.stride;
                    slots.copy_from_slice(&current.slots[source..source + current.stride]);
                    found = true;
                    break;
                }
                State::ByteRange {
                    start,
                    end,
                    next: target,
                } => {
                    if at < search.end {
                        let b = search.haystack[at];
                        if *start <= b && b <= *end {
                            let source = id as usize * current.stride;
                            scratch
                                .copy_from_slice(&current.slots[source..source + current.stride]);
                            epsilon_closure(
                                nfa,
                                stack,
                                scratch,
                                next,
                                search.haystack,
                                at + 1,
                                *target,
                                line_terminator,
                            );
                        }
                    }
                }
                State::Sparse(transitions) => {
                    if at < search.end {
                        let b = search.haystack[at];
                        if let Some(t) = transitions.iter().find(|t| t.start <= b && b <= t.end) {
                            let source = id as usize * current.stride;
                            scratch
                                .copy_from_slice(&current.slots[source..source + current.stride]);
                            epsilon_closure(
                                nfa,
                                stack,
                                scratch,
                                next,
                                search.haystack,
                                at + 1,
                                t.next,
                                line_terminator,
                            );
                        }
                    }
                }
                State::Fail | State::Look { .. } | State::Union(_) | State::Capture { .. } => {}
            }
        }
        if found {
            matched = Some(at);
            if search.earliest {
                break;
            }
        }
        core::mem::swap(current, next);
        next.set.clear();
        at += 1;
    }
    matched
}

#[allow(clippy::too_many_arguments)]
fn epsilon_closure(
    nfa: &Nfa,
    stack: &mut Vec<Frame>,
    slots: &mut [usize],
    active: &mut Active,
    haystack: &[u8],
    at: usize,
    start: StateId,
    line_terminator: u8,
) {
    stack.push(Frame::Explore(start));
    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Restore { slot, offset } => slots[slot] = offset,
            Frame::Explore(mut id) => loop {
                if !active.set.insert(id) {
                    break;
                }
                match &nfa.states[id as usize] {
                    State::Fail | State::Match | State::ByteRange { .. } | State::Sparse(_) => {
                        active.for_state(id).copy_from_slice(slots);
                        break;
                    }
                    State::Look { look, next } => {
                        if !look::matches(*look, haystack, at, line_terminator) {
                            break;
                        }
                        id = *next;
                    }
                    State::Union(alternates) => {
                        id = alternates[0];
                        stack.extend(alternates[1..].iter().rev().map(|&a| Frame::Explore(a)));
                    }
                    State::Capture { slot, next } => {
                        let slot = *slot as usize;
                        if slot < slots.len() {
                            stack.push(Frame::Restore {
                                slot,
                                offset: slots[slot],
                            });
                            slots[slot] = at;
                        }
                        id = *next;
                    }
                }
            },
        }
    }
}
