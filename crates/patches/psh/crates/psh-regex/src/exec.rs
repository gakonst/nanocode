//! Matching: an NFA simulation for the bounds of the leftmost-longest match, and a
//! backtracking search for the subexpressions inside those bounds.

use alloc::vec;
use alloc::vec::Vec;

use crate::class::{Class, INVALID, Unit, decode, decode_before, lower};
use crate::compile::{Inst, Prog};
use crate::parse::Assert;
use crate::{Captures, ExecOptions};

/// Work allowed for a search with back-references (which can take exponential time).
/// Past it the search gives up and reports no match.
const BACKTRACK_BUDGET: usize = 1 << 24;

pub(crate) fn exec(
    prog: &Prog,
    groups: usize,
    text: &[u8],
    start: usize,
    eopts: ExecOptions,
    want_groups: bool,
) -> Option<Captures> {
    let m = M {
        prog,
        text,
        start,
        eopts,
    };
    if prog.backrefs {
        return m.search_backrefs(groups);
    }
    if prog.straight && want_groups && groups > 0 {
        let mut slots = vec![None; prog.slots];
        let (s, e) = m.line(&mut slots)?;
        let mut caps: Captures = vec![None; groups + 1];
        caps[0] = Some((s, e));
        fill(&mut caps, &slots);
        return Some(caps);
    }
    let (s, e) = m.span()?;
    let mut caps: Captures = vec![None; groups + 1];
    caps[0] = Some((s, e));
    if want_groups && groups > 0 {
        let mut budget = usize::MAX;
        if let Some(slots) = m.backtrack(s, Target::Exact(e), true, &mut budget) {
            fill(&mut caps, &slots);
        }
    }
    Some(caps)
}

/// The bounds of the leftmost-longest match at `start` or later, without its
/// subexpressions and, for a pattern without back-references, without their vector.
pub(crate) fn find(
    prog: &Prog,
    groups: usize,
    text: &[u8],
    start: usize,
    eopts: ExecOptions,
) -> Option<(usize, usize)> {
    let m = M {
        prog,
        text,
        start,
        eopts,
    };
    if prog.backrefs {
        return m.search_backrefs(groups).and_then(|c| c[0]);
    }
    m.span()
}

/// Where `needle` first is in `hay`.
fn locate(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let Some((&first, rest)) = needle.split_first() else {
        return Some(0);
    };
    let mut from = 0;
    while let Some(i) = hay[from..].iter().position(|&b| b == first) {
        let at = from + i;
        if hay[at + 1..].starts_with(rest) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

fn fill(caps: &mut Captures, slots: &[Option<usize>]) {
    for (k, cap) in caps.iter_mut().enumerate().skip(1) {
        if let (Some(a), Some(b)) = (slots[2 * k], slots[2 * k + 1]) {
            *cap = Some((a, b));
        }
    }
}

struct M<'a> {
    prog: &'a Prog,
    text: &'a [u8],
    /// Where the search starts; assertions still see the text before it.
    start: usize,
    eopts: ExecOptions,
}

/// A set of threads: program counters with the start of their match, in priority
/// order, at most one per counter.
struct Threads {
    dense: Vec<(usize, usize)>,
    sparse: Vec<usize>,
}

impl Threads {
    fn new(n: usize) -> Threads {
        Threads {
            dense: Vec::with_capacity(n),
            sparse: vec![usize::MAX; n],
        }
    }

    fn contains(&self, pc: usize) -> bool {
        let i = self.sparse[pc];
        i < self.dense.len() && self.dense[i].0 == pc
    }

    fn insert(&mut self, pc: usize, start: usize) {
        self.sparse[pc] = self.dense.len();
        self.dense.push((pc, start));
    }

    fn clear(&mut self) {
        self.dense.clear();
    }
}

enum Target {
    /// The match must end here.
    Exact(usize),
    /// Find the longest match (explores every path).
    Longest,
}

enum Job {
    Explore(usize, usize),
    Slot(usize, Option<usize>),
    Reg(usize, usize),
}

impl M<'_> {
    fn utf8(&self) -> bool {
        self.prog.opts.utf8
    }

    fn is_word(&self, u: Option<Unit>) -> bool {
        u.is_some_and(|u| Class::Word.has(u, self.utf8()))
    }

    fn assert(&self, a: Assert, pos: usize) -> bool {
        let text = self.text;
        let newline = self.prog.opts.newline;
        match a {
            Assert::Bol => {
                (pos == 0 && !self.eopts.not_bol) || (newline && pos > 0 && text[pos - 1] == b'\n')
            }
            Assert::Eol => {
                (pos == text.len() && !self.eopts.not_eol)
                    || (newline && pos < text.len() && text[pos] == b'\n')
            }
            Assert::BufStart => pos == 0,
            Assert::BufEnd => pos == text.len(),
            _ => {
                let before = self.is_word(decode_before(text, pos, self.utf8()));
                let after =
                    pos < text.len() && self.is_word(Some(decode(text, pos, self.utf8()).0));
                match a {
                    Assert::WordBoundary => before != after,
                    Assert::NotWordBoundary => before == after,
                    Assert::WordStart => !before && after,
                    _ => before && !after,
                }
            }
        }
    }

    /// Whether the consuming instruction `inst` matches the char `u`.
    fn consumes(&self, inst: &Inst, u: Unit) -> bool {
        let opts = &self.prog.opts;
        match inst {
            Inst::Lit(c) => *c == u,
            Inst::Fold(c) => u < INVALID && lower(u, opts.utf8) == *c,
            Inst::Any => u < INVALID && !(opts.newline && u == b'\n' as Unit),
            Inst::Set(i) => self.prog.sets[*i].matches(u, opts.icase, opts.newline, opts.utf8),
            _ => false,
        }
    }

    /// Adds `pc` and everything reachable from it without consuming input at `pos`.
    fn add(
        &self,
        list: &mut Threads,
        pc: usize,
        start: usize,
        pos: usize,
        best: &mut Option<(usize, usize)>,
        stack: &mut Vec<usize>,
    ) {
        stack.push(pc);
        while let Some(pc) = stack.pop() {
            if list.contains(pc) {
                continue;
            }
            list.insert(pc, start);
            match &self.prog.insts[pc] {
                Inst::Jmp(t) => stack.push(*t),
                Inst::Split(a, b) => {
                    stack.push(*b);
                    stack.push(*a);
                }
                Inst::Save(_) | Inst::ProgressStart(_) | Inst::ProgressCheck(_) => {
                    stack.push(pc + 1)
                }
                Inst::Assert(a) => {
                    if self.assert(*a, pos) {
                        stack.push(pc + 1);
                    }
                }
                Inst::Match => {
                    let better = match *best {
                        None => true,
                        Some((bs, be)) => start < bs || (start == bs && pos > be),
                    };
                    if better {
                        *best = Some((start, pos));
                    }
                }
                _ => {}
            }
        }
    }

    /// The leftmost-longest match of a pattern without back-references: nothing at
    /// all when the bytes every match starts with are not in the text.
    fn span(&self) -> Option<(usize, usize)> {
        let prefix = &self.prog.prefix;
        let rest = self.text.get(self.start..)?;
        if !self.prog.anchored && !prefix.is_empty() {
            locate(rest, prefix)?;
        }
        if self.prog.straight {
            return self.line(&mut []);
        }
        self.bounds()
    }

    /// The match of a pattern with one path: the first start the path matches from.
    /// The subexpressions it passes are recorded in `slots` (none when it is empty).
    fn line(&self, slots: &mut [Option<usize>]) -> Option<(usize, usize)> {
        let first = if self.prog.anchored {
            None
        } else {
            self.prog.prefix.first().copied()
        };
        let mut start = self.start;
        if let Some(held) = self.prog.tail {
            // One byte a character over the last `held` bytes: the match is those.
            let from = self.text.len().checked_sub(held)?;
            if !self.utf8() || self.text[from..].is_ascii() {
                if from < start {
                    return None;
                }
                return self.path(from, slots).map(|end| (from, end));
            }
        }
        loop {
            if let Some(first) = first {
                start += self.text.get(start..)?.iter().position(|&b| b == first)?;
            }
            if let Some(end) = self.path(start, slots) {
                return Some((start, end));
            }
            if self.prog.anchored || start >= self.text.len() {
                return None;
            }
            start += decode(self.text, start, self.utf8()).1;
        }
    }

    /// Where the one path of the program ends when it matches from `start`.
    fn path(&self, start: usize, slots: &mut [Option<usize>]) -> Option<usize> {
        let mut pos = start;
        for inst in &self.prog.insts {
            match inst {
                Inst::Match => return Some(pos),
                Inst::Assert(a) => {
                    if !self.assert(*a, pos) {
                        return None;
                    }
                }
                Inst::Save(k) => {
                    if let Some(slot) = slots.get_mut(*k) {
                        *slot = Some(pos);
                    }
                }
                _ => {
                    if pos >= self.text.len() {
                        return None;
                    }
                    let (u, len) = decode(self.text, pos, self.utf8());
                    if !self.consumes(inst, u) {
                        return None;
                    }
                    pos += len;
                }
            }
        }
        None
    }

    /// The leftmost-longest match, by simulating all threads at once: a thread
    /// remembers where its match started, an earlier start wins a state, and once a
    /// match is found only threads with its start go on (to make it longer).
    fn bounds(&self) -> Option<(usize, usize)> {
        let n = self.prog.insts.len();
        let mut clist = Threads::new(n);
        let mut nlist = Threads::new(n);
        let mut stack = Vec::new();
        let mut best: Option<(usize, usize)> = None;
        let mut pos = self.start;
        let first = if self.prog.anchored {
            None
        } else {
            self.prog.prefix.first().copied()
        };
        loop {
            // No thread is alive and no match is held: the next one starts where the
            // first byte of every match is.
            if let Some(first) = first
                && best.is_none()
                && clist.dense.is_empty()
            {
                match self.text[pos..].iter().position(|&b| b == first) {
                    Some(i) => pos += i,
                    None => break,
                }
            }
            if best.is_none() && (pos == self.start || !self.prog.anchored) {
                self.add(&mut clist, 0, pos, pos, &mut best, &mut stack);
            }
            if pos >= self.text.len() {
                break;
            }
            if clist.dense.is_empty() && (best.is_some() || self.prog.anchored) {
                break;
            }
            let (u, len) = decode(self.text, pos, self.utf8());
            let next = pos + len;
            nlist.clear();
            for i in 0..clist.dense.len() {
                let (pc, start) = clist.dense[i];
                if best.is_some_and(|(bs, _)| start > bs) {
                    continue;
                }
                let inst = &self.prog.insts[pc];
                if self.consumes(inst, u) {
                    self.add(&mut nlist, pc + 1, start, next, &mut best, &mut stack);
                }
            }
            core::mem::swap(&mut clist, &mut nlist);
            pos = next;
        }
        best
    }

    /// A backtracking search from `start` that tries paths in program order (the
    /// preferred branch of every split first). With `memo`, no (instruction, position)
    /// pair is explored twice, which keeps it linear but is only sound without
    /// back-references. Returns the capture slots of the first path that ends as
    /// `target` wants (for `Longest`: of the longest).
    fn backtrack(
        &self,
        start: usize,
        target: Target,
        memo: bool,
        budget: &mut usize,
    ) -> Option<Vec<Option<usize>>> {
        let prog = self.prog;
        let text = self.text;
        let n = prog.insts.len();
        let span_end = match target {
            Target::Exact(e) => e,
            Target::Longest => text.len(),
        };
        let width = span_end - start + 1;
        let mut visited = if memo {
            vec![0u64; (n * width).div_ceil(64)]
        } else {
            Vec::new()
        };
        let mut slots: Vec<Option<usize>> = vec![None; prog.slots];
        let mut regs: Vec<usize> = vec![usize::MAX; prog.regs];
        let mut best: Option<(usize, Vec<Option<usize>>)> = None;
        let mut jobs = vec![Job::Explore(0, start)];
        while let Some(job) = jobs.pop() {
            let (mut pc, mut pos) = match job {
                Job::Slot(k, v) => {
                    slots[k] = v;
                    continue;
                }
                Job::Reg(r, v) => {
                    regs[r] = v;
                    continue;
                }
                Job::Explore(pc, pos) => (pc, pos),
            };
            loop {
                if *budget == 0 {
                    return None;
                }
                *budget -= 1;
                if pos > span_end {
                    break;
                }
                if memo {
                    let key = pc * width + (pos - start);
                    let (w, b) = (key / 64, 1u64 << (key % 64));
                    if visited[w] & b != 0 {
                        break;
                    }
                    visited[w] |= b;
                }
                match &prog.insts[pc] {
                    Inst::Match => match target {
                        Target::Exact(e) => {
                            if pos == e {
                                slots[0] = Some(start);
                                slots[1] = Some(pos);
                                return Some(slots);
                            }
                            break;
                        }
                        Target::Longest => {
                            if best.as_ref().is_none_or(|(e, _)| pos > *e) {
                                let mut s = slots.clone();
                                s[0] = Some(start);
                                s[1] = Some(pos);
                                best = Some((pos, s));
                            }
                            break;
                        }
                    },
                    inst @ (Inst::Lit(_) | Inst::Fold(_) | Inst::Any | Inst::Set(_)) => {
                        if pos >= text.len() {
                            break;
                        }
                        let (u, len) = decode(text, pos, self.utf8());
                        if !self.consumes(inst, u) {
                            break;
                        }
                        pos += len;
                        pc += 1;
                    }
                    Inst::Assert(a) => {
                        if !self.assert(*a, pos) {
                            break;
                        }
                        pc += 1;
                    }
                    Inst::Save(k) => {
                        jobs.push(Job::Slot(*k, slots[*k]));
                        slots[*k] = Some(pos);
                        pc += 1;
                    }
                    Inst::Split(a, b) => {
                        jobs.push(Job::Explore(*b, pos));
                        pc = *a;
                    }
                    Inst::Jmp(t) => pc = *t,
                    Inst::ProgressStart(r) => {
                        jobs.push(Job::Reg(*r, regs[*r]));
                        regs[*r] = pos;
                        pc += 1;
                    }
                    Inst::ProgressCheck(r) => {
                        if regs[*r] == pos {
                            break;
                        }
                        pc += 1;
                    }
                    Inst::Backref(k) => {
                        let (Some(a), Some(b)) = (slots[2 * k], slots[2 * k + 1]) else {
                            break;
                        };
                        let Some(len) = self.backref_len(a, b, pos) else {
                            break;
                        };
                        pos += len;
                        pc += 1;
                    }
                }
            }
        }
        best.map(|(_, s)| s)
    }

    /// The length of the text at `pos` that repeats `text[a..b]` (case-insensitively
    /// under `icase`), if it does.
    fn backref_len(&self, a: usize, b: usize, pos: usize) -> Option<usize> {
        let text = self.text;
        if !self.prog.opts.icase {
            let n = b - a;
            return (pos + n <= text.len() && text[a..b] == text[pos..pos + n]).then_some(n);
        }
        let utf8 = self.utf8();
        let (mut i, mut j) = (a, pos);
        while i < b {
            if j >= text.len() {
                return None;
            }
            let (u, n) = decode(text, i, utf8);
            let (v, m) = decode(text, j, utf8);
            if u != v && lower(u, utf8) != lower(v, utf8) {
                return None;
            }
            i += n;
            j += m;
        }
        Some(j - pos)
    }

    /// With back-references: for each start, the longest match by exhaustive search,
    /// then its subexpressions as the first path in program order that ends there.
    fn search_backrefs(&self, groups: usize) -> Option<Captures> {
        let mut budget = BACKTRACK_BUDGET;
        let mut start = self.start;
        loop {
            if let Some(slots) = self.backtrack(start, Target::Longest, false, &mut budget) {
                let end = slots[1].unwrap();
                let slots = self
                    .backtrack(start, Target::Exact(end), false, &mut budget)
                    .unwrap_or(slots);
                let mut caps: Captures = vec![None; groups + 1];
                caps[0] = Some((start, end));
                fill(&mut caps, &slots);
                return Some(caps);
            }
            if budget == 0 || start >= self.text.len() || self.prog.anchored {
                return None;
            }
            start += decode(self.text, start, self.utf8()).1;
        }
    }
}
