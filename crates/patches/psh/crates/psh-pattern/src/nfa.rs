use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;

use crate::class::{ClassBytes, ClassUnicode};
use crate::hir::{Class, Hir, HirKind, Look, LookSet};
use crate::utf8;

pub type StateId = u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Transition {
    pub start: u8,
    pub end: u8,
    pub next: StateId,
}

#[derive(Clone, Debug)]
pub enum State {
    ByteRange { start: u8, end: u8, next: StateId },
    Sparse(Box<[Transition]>),
    Look { look: Look, next: StateId },
    Union(Box<[StateId]>),
    Capture { slot: u32, next: StateId },
    Fail,
    Match,
}

#[derive(Clone, Debug)]
enum Building {
    Empty { next: StateId },
    ByteRange { start: u8, end: u8, next: StateId },
    Sparse { transitions: Vec<Transition> },
    Look { look: Look, next: StateId },
    Union { alternates: Vec<StateId> },
    UnionReverse { alternates: Vec<StateId> },
    CaptureStart { group: u32, next: StateId },
    CaptureEnd { group: u32, next: StateId },
    Fail,
    Match,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TooBig {
    pub limit: usize,
}

#[derive(Clone, Copy)]
struct Fragment {
    start: StateId,
    end: StateId,
}

#[derive(Clone, Debug)]
pub struct Nfa {
    pub states: Vec<State>,
    pub start_anchored: StateId,
    pub start_unanchored: StateId,
    pub group_names: Vec<Option<String>>,
    pub look_set: LookSet,
    pub has_empty: bool,
    pub reverse: bool,
    pub always_anchored: bool,
}

pub struct CompileOptions {
    pub reverse: bool,
    pub size_limit: Option<usize>,
}

const UTF8_COMPILED_CAPACITY_ENTRIES: usize = 10_000;
const UTF8_SUFFIX_CAPACITY_ENTRIES: usize = 1000;
const FNV_PRIME: u64 = 1099511628211;
const FNV_INIT: u64 = 14695981039346656037;

#[derive(Clone, Default)]
struct CompiledEntry {
    version: u16,
    key: Vec<Transition>,
    value: StateId,
}

#[derive(Clone, Copy, Default)]
struct SuffixEntry {
    version: u16,
    key: (StateId, u8, u8),
    value: StateId,
}

struct Utf8Node {
    transitions: Vec<Transition>,
    last: Option<(u8, u8)>,
}

impl Utf8Node {
    fn set_last_transition(&mut self, next: StateId) {
        if let Some((start, end)) = self.last.take() {
            self.transitions.push(Transition { start, end, next });
        }
    }
}

#[derive(Default)]
struct TrieState {
    transitions: Vec<(u8, usize)>,
    chunks: Vec<(usize, usize)>,
}

impl TrieState {
    fn active_start(&self) -> usize {
        self.chunks.last().map_or(0, |&(_, end)| end)
    }

    fn chunk(&self, index: usize) -> Option<&[(u8, usize)]> {
        match self.chunks.get(index) {
            Some(&(start, end)) => Some(&self.transitions[start..end]),
            None if index == self.chunks.len() => Some(&self.transitions[self.active_start()..]),
            None => None,
        }
    }
}

struct TrieFrame {
    state: usize,
    chunk: usize,
    position: usize,
    union: Vec<StateId>,
    sparse: Vec<Transition>,
}

impl TrieFrame {
    fn new(state: usize) -> TrieFrame {
        TrieFrame {
            state,
            chunk: 0,
            position: 0,
            union: Vec::new(),
            sparse: Vec::new(),
        }
    }
}

fn fnv_compiled(key: &[Transition]) -> usize {
    let mut hash = FNV_INIT;
    for transition in key {
        hash = (hash ^ u64::from(transition.start)).wrapping_mul(FNV_PRIME);
        hash = (hash ^ u64::from(transition.end)).wrapping_mul(FNV_PRIME);
        hash = (hash ^ u64::from(transition.next)).wrapping_mul(FNV_PRIME);
    }
    usize::try_from(hash % UTF8_COMPILED_CAPACITY_ENTRIES as u64)
        .expect("a slot index fits in usize")
}

fn fnv_suffix(key: (StateId, u8, u8)) -> usize {
    let mut hash = FNV_INIT;
    hash = (hash ^ u64::from(key.0)).wrapping_mul(FNV_PRIME);
    hash = (hash ^ u64::from(key.1)).wrapping_mul(FNV_PRIME);
    hash = (hash ^ u64::from(key.2)).wrapping_mul(FNV_PRIME);
    usize::try_from(hash % UTF8_SUFFIX_CAPACITY_ENTRIES as u64).expect("a slot index fits in usize")
}

struct Compiler {
    states: Vec<Building>,
    memory_extra: usize,
    size_limit: Option<usize>,
    reverse: bool,
    compiled: Vec<CompiledEntry>,
    compiled_version: u16,
    suffixes: Vec<SuffixEntry>,
    suffixes_version: u16,
    captures: bool,
}

pub fn compile(hir: &Hir, options: CompileOptions) -> Result<Nfa, TooBig> {
    let mut compiler = Compiler::new(options.reverse, options.size_limit, true);
    compiler.check()?;
    let always_anchored = if options.reverse {
        hir.properties().look_set_suffix.contains(Look::End)
    } else {
        hir.properties().look_set_prefix.contains(Look::Start)
    };
    let prefix = if always_anchored {
        compiler.c_empty()?
    } else {
        compiler.c_at_least(&Hir::dot(crate::hir::Dot::AnyByte), false, 0)?
    };
    let one = if options.reverse {
        compiler.c(hir)?
    } else {
        compiler.c_capture(0, hir)?
    };
    let matched = compiler.add(Building::Match)?;
    compiler.patch(one.end, matched)?;
    compiler.patch(prefix.end, one.start)?;
    let mut group_names = Vec::new();
    collect_group_names(hir, &mut group_names);
    Ok(compiler.finish(one.start, prefix.start, group_names, always_anchored))
}

pub fn check_set(hirs: &[Hir], reverse: bool, size_limit: usize) -> Result<(), TooBig> {
    let mut compiler = Compiler::new(reverse, Some(size_limit), false);
    compiler.check()?;
    let all_anchored = hirs.iter().all(|hir| {
        let properties = hir.properties();
        if reverse {
            properties.look_set_suffix.contains(Look::End)
        } else {
            properties.look_set_prefix.contains(Look::Start)
        }
    });
    let prefix = if all_anchored {
        compiler.c_empty()?
    } else {
        compiler.c_at_least(&Hir::dot(crate::hir::Dot::AnyByte), false, 0)?
    };
    let one = |compiler: &mut Compiler, hir: &Hir| -> Result<Fragment, TooBig> {
        let compiled = compiler.c(hir)?;
        let matched = compiler.add(Building::Match)?;
        compiler.patch(compiled.end, matched)?;
        Ok(Fragment {
            start: compiled.start,
            end: matched,
        })
    };
    let all = match hirs {
        [] => compiler.c_fail()?,
        [only] => one(&mut compiler, only)?,
        [first, second, rest @ ..] => {
            let first = one(&mut compiler, first)?;
            let second = one(&mut compiler, second)?;
            let union = compiler.union(true)?;
            let end = compiler.add(Building::Empty { next: 0 })?;
            compiler.patch(union, first.start)?;
            compiler.patch(first.end, end)?;
            compiler.patch(union, second.start)?;
            compiler.patch(second.end, end)?;
            for hir in rest {
                let compiled = one(&mut compiler, hir)?;
                compiler.patch(union, compiled.start)?;
                compiler.patch(compiled.end, end)?;
            }
            Fragment { start: union, end }
        }
    };
    compiler.patch(prefix.end, all.start)
}

fn collect_group_names(hir: &Hir, names: &mut Vec<Option<String>>) {
    if names.is_empty() {
        names.push(None);
    }
    match hir.kind() {
        HirKind::Capture(capture) => {
            let index = capture.index as usize;
            if names.len() <= index {
                names.resize(index + 1, None);
            }
            names[index].clone_from(&capture.name);
            collect_group_names(&capture.sub, names);
        }
        HirKind::Repetition(repetition) => collect_group_names(&repetition.sub, names),
        HirKind::Concat(subs) | HirKind::Alternation(subs) => {
            for sub in subs {
                collect_group_names(sub, names);
            }
        }
        _ => {}
    }
}

impl Compiler {
    fn new(reverse: bool, size_limit: Option<usize>, captures: bool) -> Compiler {
        Compiler {
            states: Vec::new(),
            memory_extra: 0,
            size_limit,
            reverse,
            compiled: Vec::new(),
            compiled_version: 0,
            suffixes: Vec::new(),
            suffixes_version: 0,
            captures,
        }
    }

    fn memory_usage(&self) -> usize {
        self.states.len() * size_of::<Building>() + self.memory_extra
    }

    fn check(&self) -> Result<(), TooBig> {
        match self.size_limit {
            Some(limit) if self.memory_usage() > limit => Err(TooBig { limit }),
            _ => Ok(()),
        }
    }

    fn add(&mut self, state: Building) -> Result<StateId, TooBig> {
        self.memory_extra += match &state {
            Building::Sparse { transitions } => transitions.len() * size_of::<Transition>(),
            Building::Union { alternates } | Building::UnionReverse { alternates } => {
                alternates.len() * size_of::<StateId>()
            }
            _ => 0,
        };
        let id = StateId::try_from(self.states.len()).expect("state identifiers fit in 32 bits");
        self.states.push(state);
        self.check()?;
        Ok(id)
    }

    fn patch(&mut self, from: StateId, to: StateId) -> Result<(), TooBig> {
        match &mut self.states[from as usize] {
            Building::Empty { next }
            | Building::ByteRange { next, .. }
            | Building::Look { next, .. }
            | Building::CaptureStart { next, .. }
            | Building::CaptureEnd { next, .. } => *next = to,
            Building::Union { alternates } | Building::UnionReverse { alternates } => {
                alternates.push(to);
                self.memory_extra += size_of::<StateId>();
                self.check()?;
            }
            Building::Sparse { .. } => panic!("cannot patch from a sparse state"),
            Building::Fail | Building::Match => {}
        }
        Ok(())
    }

    fn union(&mut self, greedy: bool) -> Result<StateId, TooBig> {
        if greedy {
            self.add(Building::Union {
                alternates: Vec::new(),
            })
        } else {
            self.add(Building::UnionReverse {
                alternates: Vec::new(),
            })
        }
    }

    fn c_empty(&mut self) -> Result<Fragment, TooBig> {
        let id = self.add(Building::Empty { next: 0 })?;
        Ok(Fragment { start: id, end: id })
    }

    fn c_fail(&mut self) -> Result<Fragment, TooBig> {
        let id = self.add(Building::Fail)?;
        Ok(Fragment { start: id, end: id })
    }

    fn c_range(&mut self, start: u8, end: u8) -> Result<Fragment, TooBig> {
        let id = self.add(Building::ByteRange {
            start,
            end,
            next: 0,
        })?;
        Ok(Fragment { start: id, end: id })
    }

    fn c(&mut self, hir: &Hir) -> Result<Fragment, TooBig> {
        match hir.kind() {
            HirKind::Empty => self.c_empty(),
            HirKind::Literal(bytes) => self.c_concat(bytes.len(), |compiler, index| {
                compiler.c_range(bytes[index], bytes[index])
            }),
            HirKind::Class(Class::Bytes(class)) => self.c_byte_class(class),
            HirKind::Class(Class::Unicode(class)) => self.c_unicode_class(class),
            HirKind::Look(look) => {
                let id = self.add(Building::Look {
                    look: *look,
                    next: 0,
                })?;
                Ok(Fragment { start: id, end: id })
            }
            HirKind::Repetition(repetition) => match (repetition.min, repetition.max) {
                (0, Some(1)) => self.c_zero_or_one(&repetition.sub, repetition.greedy),
                (min, None) => self.c_at_least(&repetition.sub, repetition.greedy, min),
                (min, Some(max)) if min == max => self.c_exactly(&repetition.sub, min),
                (min, Some(max)) => self.c_bounded(&repetition.sub, repetition.greedy, min, max),
            },
            HirKind::Capture(capture) => {
                if self.reverse || !self.captures {
                    self.c(&capture.sub)
                } else {
                    self.c_capture(capture.index, &capture.sub)
                }
            }
            HirKind::Concat(subs) => {
                self.c_concat(subs.len(), |compiler, index| compiler.c(&subs[index]))
            }
            HirKind::Alternation(subs) => self.c_alternation(subs),
        }
    }

    fn c_concat(
        &mut self,
        count: usize,
        mut compile: impl FnMut(&mut Compiler, usize) -> Result<Fragment, TooBig>,
    ) -> Result<Fragment, TooBig> {
        let reverse = self.reverse;
        let order = |position: usize| {
            if reverse {
                count - 1 - position
            } else {
                position
            }
        };
        if count == 0 {
            return self.c_empty();
        }
        let first = compile(self, order(0))?;
        let mut end = first.end;
        for position in 1..count {
            let next = compile(self, order(position))?;
            self.patch(end, next.start)?;
            end = next.end;
        }
        Ok(Fragment {
            start: first.start,
            end,
        })
    }

    fn c_alternation(&mut self, subs: &[Hir]) -> Result<Fragment, TooBig> {
        let literals = subs
            .iter()
            .filter(|sub| matches!(sub.kind(), HirKind::Literal(_)))
            .count();
        if literals > 1 && literals == subs.len() {
            return self.c_literal_trie(subs);
        }
        let Some((first, rest)) = subs.split_first() else {
            return self.c_fail();
        };
        let first = self.c(first)?;
        let Some((second, rest)) = rest.split_first() else {
            return Ok(first);
        };
        let second = self.c(second)?;
        let union = self.union(true)?;
        let end = self.add(Building::Empty { next: 0 })?;
        self.patch(union, first.start)?;
        self.patch(first.end, end)?;
        self.patch(union, second.start)?;
        self.patch(second.end, end)?;
        for sub in rest {
            let compiled = self.c(sub)?;
            self.patch(union, compiled.start)?;
            self.patch(compiled.end, end)?;
        }
        Ok(Fragment { start: union, end })
    }

    fn c_literal_trie(&mut self, subs: &[Hir]) -> Result<Fragment, TooBig> {
        let mut trie = vec![TrieState::default()];
        for sub in subs {
            let HirKind::Literal(bytes) = sub.kind() else {
                panic!("a literal trie holds literals only")
            };
            let mut previous = 0;
            for position in 0..bytes.len() {
                let byte = if self.reverse {
                    bytes[bytes.len() - 1 - position]
                } else {
                    bytes[position]
                };
                let active = trie[previous].active_start();
                previous = match trie[previous].transitions[active..]
                    .binary_search_by_key(&byte, |&(b, _)| b)
                {
                    Ok(index) => trie[previous].transitions[active + index].1,
                    Err(index) => {
                        let next = trie.len();
                        trie.push(TrieState::default());
                        trie[previous]
                            .transitions
                            .insert(active + index, (byte, next));
                        next
                    }
                };
            }
            let state = &mut trie[previous];
            if state.transitions.is_empty() && !state.chunks.is_empty() {
                continue;
            }
            let start = state.active_start();
            let end = state.transitions.len();
            state.chunks.push((start, end));
        }
        self.c_trie_states(&trie)
    }

    fn c_trie_states(&mut self, trie: &[TrieState]) -> Result<Fragment, TooBig> {
        let end = self.add(Building::Empty { next: 0 })?;
        let mut stack: Vec<TrieFrame> = Vec::new();
        let mut frame = TrieFrame::new(0);
        loop {
            let chunk = trie[frame.state]
                .chunk(frame.chunk)
                .expect("a frame reads an existing chunk");
            if let Some(&(byte, next)) = chunk.get(frame.position) {
                frame.position += 1;
                if trie[next].transitions.is_empty() {
                    frame.sparse.push(Transition {
                        start: byte,
                        end: byte,
                        next: end,
                    });
                } else {
                    frame.sparse.push(Transition {
                        start: byte,
                        end: byte,
                        next: 0,
                    });
                    stack.push(frame);
                    frame = TrieFrame::new(next);
                }
                continue;
            }
            if !frame.sparse.is_empty() {
                let id = if frame.sparse.len() == 1 {
                    let only = frame.sparse.pop().expect("one transition");
                    self.add(Building::ByteRange {
                        start: only.start,
                        end: only.end,
                        next: only.next,
                    })?
                } else {
                    self.add(Building::Sparse {
                        transitions: core::mem::take(&mut frame.sparse),
                    })?
                };
                frame.union.push(id);
            }
            if trie[frame.state].chunk(frame.chunk + 1).is_some() {
                frame.union.push(end);
                frame.chunk += 1;
                frame.position = 0;
                continue;
            }
            let start = self.add(Building::Union {
                alternates: core::mem::take(&mut frame.union),
            })?;
            match stack.pop() {
                None => return Ok(Fragment { start, end }),
                Some(mut parent) => {
                    parent
                        .sparse
                        .last_mut()
                        .expect("the parent's last transition leads here")
                        .next = start;
                    frame = parent;
                }
            }
        }
    }

    fn c_capture(&mut self, group: u32, hir: &Hir) -> Result<Fragment, TooBig> {
        let start = self.add(Building::CaptureStart { group, next: 0 })?;
        let inner = self.c(hir)?;
        let end = self.add(Building::CaptureEnd { group, next: 0 })?;
        self.patch(start, inner.start)?;
        self.patch(inner.end, end)?;
        Ok(Fragment { start, end })
    }

    fn c_exactly(&mut self, hir: &Hir, n: u32) -> Result<Fragment, TooBig> {
        self.c_concat(
            usize::try_from(n).expect("a repetition count fits in usize"),
            |compiler, _| compiler.c(hir),
        )
    }

    fn c_bounded(
        &mut self,
        hir: &Hir,
        greedy: bool,
        min: u32,
        max: u32,
    ) -> Result<Fragment, TooBig> {
        let prefix = self.c_exactly(hir, min)?;
        if min == max {
            return Ok(prefix);
        }
        let empty = self.add(Building::Empty { next: 0 })?;
        let mut previous_end = prefix.end;
        for _ in min..max {
            let union = self.union(greedy)?;
            let compiled = self.c(hir)?;
            self.patch(previous_end, union)?;
            self.patch(union, compiled.start)?;
            self.patch(union, empty)?;
            previous_end = compiled.end;
        }
        self.patch(previous_end, empty)?;
        Ok(Fragment {
            start: prefix.start,
            end: empty,
        })
    }

    fn c_at_least(&mut self, hir: &Hir, greedy: bool, n: u32) -> Result<Fragment, TooBig> {
        if n == 0 {
            if hir.properties().minimum_len.is_some_and(|len| len > 0) {
                let union = self.union(greedy)?;
                let compiled = self.c(hir)?;
                self.patch(union, compiled.start)?;
                self.patch(compiled.end, union)?;
                return Ok(Fragment {
                    start: union,
                    end: union,
                });
            }
            let compiled = self.c(hir)?;
            let plus = self.union(greedy)?;
            self.patch(compiled.end, plus)?;
            self.patch(plus, compiled.start)?;
            let question = self.union(greedy)?;
            let empty = self.add(Building::Empty { next: 0 })?;
            self.patch(question, compiled.start)?;
            self.patch(question, empty)?;
            self.patch(plus, empty)?;
            Ok(Fragment {
                start: question,
                end: empty,
            })
        } else if n == 1 {
            let compiled = self.c(hir)?;
            let union = self.union(greedy)?;
            self.patch(compiled.end, union)?;
            self.patch(union, compiled.start)?;
            Ok(Fragment {
                start: compiled.start,
                end: union,
            })
        } else {
            let prefix = self.c_exactly(hir, n - 1)?;
            let last = self.c(hir)?;
            let union = self.union(greedy)?;
            self.patch(prefix.end, last.start)?;
            self.patch(last.end, union)?;
            self.patch(union, last.start)?;
            Ok(Fragment {
                start: prefix.start,
                end: union,
            })
        }
    }

    fn c_zero_or_one(&mut self, hir: &Hir, greedy: bool) -> Result<Fragment, TooBig> {
        let union = self.union(greedy)?;
        let compiled = self.c(hir)?;
        let empty = self.add(Building::Empty { next: 0 })?;
        self.patch(union, compiled.start)?;
        self.patch(union, empty)?;
        self.patch(compiled.end, empty)?;
        Ok(Fragment {
            start: union,
            end: empty,
        })
    }

    fn c_byte_class(&mut self, class: &ClassBytes) -> Result<Fragment, TooBig> {
        let end = self.add(Building::Empty { next: 0 })?;
        let transitions = class
            .ranges()
            .iter()
            .map(|r| Transition {
                start: r.start as u8,
                end: r.end as u8,
                next: end,
            })
            .collect();
        let start = self.add(Building::Sparse { transitions })?;
        Ok(Fragment { start, end })
    }

    fn c_unicode_class(&mut self, class: &ClassUnicode) -> Result<Fragment, TooBig> {
        if let Some(bytes) = class.to_byte_class() {
            return self.c_byte_class(&bytes);
        }
        if self.reverse {
            self.c_utf8_reverse(class)
        } else {
            self.c_utf8_forward(class)
        }
    }

    fn c_utf8_forward(&mut self, class: &ClassUnicode) -> Result<Fragment, TooBig> {
        let target = self.add(Building::Empty { next: 0 })?;
        if self.compiled.is_empty() {
            self.compiled = vec![CompiledEntry::default(); UTF8_COMPILED_CAPACITY_ENTRIES];
        } else {
            self.compiled_version = self.compiled_version.wrapping_add(1);
            if self.compiled_version == 0 {
                self.compiled = vec![CompiledEntry::default(); UTF8_COMPILED_CAPACITY_ENTRIES];
            }
        }
        let mut uncompiled = vec![Utf8Node {
            transitions: Vec::new(),
            last: None,
        }];
        let mut sequences = Vec::new();
        for range in class.ranges() {
            sequences.clear();
            utf8::sequences(range.start, range.end, &mut sequences);
            for sequence in &sequences {
                let ranges = sequence.as_slice();
                let shared = ranges
                    .iter()
                    .zip(&uncompiled)
                    .take_while(|(range, node)| node.last == Some((range.start, range.end)))
                    .count();
                assert!(
                    shared < ranges.len(),
                    "UTF-8 sequences of a class are disjoint"
                );
                self.utf8_compile_from(&mut uncompiled, shared, target)?;
                let top = uncompiled.last_mut().expect("the root stays");
                assert!(top.last.is_none());
                top.last = Some((ranges[shared].start, ranges[shared].end));
                for range in &ranges[shared + 1..] {
                    uncompiled.push(Utf8Node {
                        transitions: Vec::new(),
                        last: Some((range.start, range.end)),
                    });
                }
            }
        }
        self.utf8_compile_from(&mut uncompiled, 0, target)?;
        assert_eq!(uncompiled.len(), 1);
        let root = uncompiled.pop().expect("the root stays");
        assert!(root.last.is_none());
        let start = self.utf8_compile(root.transitions)?;
        Ok(Fragment { start, end: target })
    }

    fn utf8_compile_from(
        &mut self,
        uncompiled: &mut Vec<Utf8Node>,
        from: usize,
        target: StateId,
    ) -> Result<(), TooBig> {
        let mut next = target;
        while from + 1 < uncompiled.len() {
            let mut node = uncompiled.pop().expect("more than one node");
            node.set_last_transition(next);
            next = self.utf8_compile(node.transitions)?;
        }
        uncompiled
            .last_mut()
            .expect("the root stays")
            .set_last_transition(next);
        Ok(())
    }

    fn utf8_compile(&mut self, transitions: Vec<Transition>) -> Result<StateId, TooBig> {
        let slot = fnv_compiled(&transitions);
        let entry = &self.compiled[slot];
        if entry.version == self.compiled_version && entry.key == transitions {
            return Ok(entry.value);
        }
        let id = self.add(Building::Sparse {
            transitions: transitions.clone(),
        })?;
        self.compiled[slot] = CompiledEntry {
            version: self.compiled_version,
            key: transitions,
            value: id,
        };
        Ok(id)
    }

    fn c_utf8_reverse(&mut self, class: &ClassUnicode) -> Result<Fragment, TooBig> {
        if self.suffixes.is_empty() {
            self.suffixes = vec![SuffixEntry::default(); UTF8_SUFFIX_CAPACITY_ENTRIES];
        } else {
            self.suffixes_version = self.suffixes_version.wrapping_add(1);
            if self.suffixes_version == 0 {
                self.suffixes = vec![SuffixEntry::default(); UTF8_SUFFIX_CAPACITY_ENTRIES];
            }
        }
        let union = self.union(true)?;
        let alternation_end = self.add(Building::Empty { next: 0 })?;
        let mut sequences = Vec::new();
        for range in class.ranges() {
            sequences.clear();
            utf8::sequences(range.start, range.end, &mut sequences);
            for sequence in &sequences {
                let mut end = alternation_end;
                for range in sequence.as_slice() {
                    let key = (end, range.start, range.end);
                    let slot = fnv_suffix(key);
                    let entry = self.suffixes[slot];
                    if entry.version == self.suffixes_version && entry.key == key {
                        end = entry.value;
                        continue;
                    }
                    let compiled = self.c_range(range.start, range.end)?;
                    self.patch(compiled.end, end)?;
                    end = compiled.start;
                    self.suffixes[slot] = SuffixEntry {
                        version: self.suffixes_version,
                        key,
                        value: end,
                    };
                }
                self.patch(union, end)?;
            }
        }
        Ok(Fragment {
            start: union,
            end: alternation_end,
        })
    }

    fn resolve(&self, mut id: StateId) -> StateId {
        let mut steps = 0;
        loop {
            match &self.states[id as usize] {
                Building::Empty { next } => id = *next,
                Building::Union { alternates } | Building::UnionReverse { alternates }
                    if alternates.len() == 1 =>
                {
                    id = alternates[0]
                }
                _ => return id,
            }
            steps += 1;
            assert!(steps <= self.states.len(), "a cycle of empty states");
        }
    }

    fn finish(
        self,
        start_anchored: StateId,
        start_unanchored: StateId,
        group_names: Vec<Option<String>>,
        always_anchored: bool,
    ) -> Nfa {
        let mut states = Vec::with_capacity(self.states.len());
        let mut look_set = LookSet::empty();
        for state in &self.states {
            let resolved = match state {
                Building::Empty { .. } => State::Fail,
                Building::ByteRange { start, end, next } => State::ByteRange {
                    start: *start,
                    end: *end,
                    next: self.resolve(*next),
                },
                Building::Sparse { transitions } => State::Sparse(
                    transitions
                        .iter()
                        .map(|t| Transition {
                            next: self.resolve(t.next),
                            ..*t
                        })
                        .collect(),
                ),
                Building::Look { look, next } => {
                    look_set = look_set.insert(*look);
                    State::Look {
                        look: *look,
                        next: self.resolve(*next),
                    }
                }
                Building::Union { alternates } => {
                    if alternates.is_empty() {
                        State::Fail
                    } else {
                        State::Union(alternates.iter().map(|&a| self.resolve(a)).collect())
                    }
                }
                Building::UnionReverse { alternates } => {
                    if alternates.is_empty() {
                        State::Fail
                    } else {
                        State::Union(alternates.iter().rev().map(|&a| self.resolve(a)).collect())
                    }
                }
                Building::CaptureStart { group, next } => State::Capture {
                    slot: group * 2,
                    next: self.resolve(*next),
                },
                Building::CaptureEnd { group, next } => State::Capture {
                    slot: group * 2 + 1,
                    next: self.resolve(*next),
                },
                Building::Fail => State::Fail,
                Building::Match => State::Match,
            };
            states.push(resolved);
        }
        let start_anchored = self.resolve(start_anchored);
        let start_unanchored = self.resolve(start_unanchored);
        let has_empty = epsilon_reaches_match(&states, start_anchored);
        Nfa {
            states,
            start_anchored,
            start_unanchored,
            group_names,
            look_set,
            has_empty,
            reverse: self.reverse,
            always_anchored,
        }
    }
}

fn epsilon_reaches_match(states: &[State], start: StateId) -> bool {
    let mut seen = vec![false; states.len()];
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if core::mem::replace(&mut seen[id as usize], true) {
            continue;
        }
        match &states[id as usize] {
            State::Match => return true,
            State::Look { next, .. } | State::Capture { next, .. } => stack.push(*next),
            State::Union(alternates) => stack.extend(alternates.iter().copied()),
            State::ByteRange { .. } | State::Sparse(_) | State::Fail => {}
        }
    }
    false
}
