//! The backtracking matcher.
//!
//! All state is integers: the program counter, the position, capture and register slots,
//! and a backtrack stack of `u32` words. The stack holds frames (`pc, pos, prev_bp << 2 |
//! kind, generation`, pushed at choice points and lookarounds) with an undo log of slot
//! writes above each frame (`slot, old value, old slot_gen` triples). A slot is logged at
//! most once per frame: `slot_gen` records the generation of the frame that last logged
//! it, and undoing an entry restores that too, so writes under a frame whose inner frames
//! were popped are not logged again (a `STAR` retry writing its count under the frame
//! below would otherwise grow the stack by an entry per retry). Writes with no frame at
//! all are not logged: a search runs each attempt on an empty stack and clears the
//! captures before the next (D3135). Nothing lives on the Rust stack between
//! instructions, so running out of fuel at any instruction (or while looking for the next
//! start) returns with the [`ExecState`] holding everything, and [`resume_in`] continues
//! exactly where it stopped.
use crate::PushOut;

use alloc::vec::Vec;

use super::Program;
use super::charset::{canonicalize, is_line_terminator, is_word_char};
use super::compiler::*;

/// Slot value of an undefined capture.
const UNSET: u32 = u32::MAX;

/// Frame kinds.
const K_SPLIT: u32 = 0;
const K_LOOK: u32 = 1;
const K_NEG_LOOK: u32 = 2;
/// A `STAR` loop that can give back (greedy) or take (lazy) one more character; its pc
/// is the `STAR` instruction.
const K_STAR: u32 = 3;
const FRAME: usize = 4;

/// The `pc` of a search between attempts: looking for the next start from `pos`.
const PC_FIND: u32 = u32::MAX;

/// A program's words read in place: little-endian byte quads, as a heap cell holds them
/// (so an exec reads the RegExp's cell without copying it, D3135).
#[derive(Clone, Copy, Debug)]
pub struct Prog<'a>(&'a [[u8; 4]]);

impl<'a> Prog<'a> {
    pub fn new(words: &'a [[u8; 4]]) -> Prog<'a> {
        Prog(words)
    }

    #[inline(always)]
    fn at(self, i: usize) -> u32 {
        u32::from_le_bytes(self.0[i])
    }

    #[inline(always)]
    fn sub(self, at: usize, len: usize) -> Prog<'a> {
        Prog(&self.0[at..at + len])
    }

    pub fn flags(self) -> super::Flags {
        super::Flags::from_bits(self.at(H_FLAGS) as u16)
    }

    /// Number of capture groups, including group 0.
    pub fn capture_count(self) -> usize {
        self.at(H_CAPTURES) as usize
    }

    /// Whether any group has a name.
    pub fn has_group_names(self) -> bool {
        self.at(H_MODE) & M_NAMES != 0
    }

    /// The name of capture group `index` (UTF-16 code units), if it has one.
    pub fn group_name(self, index: usize) -> Option<impl Iterator<Item = u16> + 'a> {
        let count = self.capture_count();
        if index == 0 || index >= count {
            return None;
        }
        let mut p = H_LEN;
        for _ in 1..index {
            p += 1 + self.at(p) as usize;
        }
        let len = self.at(p) as usize;
        (len > 0).then(|| {
            self.0[p + 1..p + 1 + len]
                .iter()
                .map(|w| w[0] as u16 | (w[1] as u16) << 8)
        })
    }

    /// Whether the name of group `index` is `name`.
    pub fn group_name_is(self, index: usize, name: &[u16]) -> bool {
        self.group_name(index)
            .is_some_and(|n| n.eq(name.iter().copied()))
    }
}

/// Binary search in sorted inclusive `(lo, hi)` pairs.
fn in_ranges(ranges: Prog<'_>, c: u32) -> bool {
    let (mut lo, mut hi) = (0, ranges.0.len() / 2);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if c < ranges.at(2 * mid) {
            hi = mid;
        } else if c > ranges.at(2 * mid + 1) {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

/// The instruction after the `STAR` at `pc` (D1501).
fn star_next(code: Prog<'_>, pc: usize) -> u32 {
    let lead = (code.at(pc) & F_LEAD != 0) as usize;
    (pc + 6 + code.at(pc + 4) as usize + 2 * code.at(pc + 5) as usize + lead) as u32
}

/// The string being matched.
#[derive(Clone, Copy, Debug)]
pub enum Input<'a> {
    Latin1(&'a [u8]),
    Utf16(&'a [u16]),
    /// UTF-16 as little-endian byte pairs: a two-byte heap string, read in place.
    Utf16Le(&'a [u8]),
}

impl Input<'_> {
    pub fn len(&self) -> usize {
        match self {
            Input::Latin1(s) => s.len(),
            Input::Utf16(s) => s.len(),
            Input::Utf16Le(s) => s.len() / 2,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn unit(&self, i: usize) -> u32 {
        match self {
            Input::Latin1(s) => s[i] as u32,
            Input::Utf16(s) => s[i] as u32,
            Input::Utf16Le(s) => u16::from_le_bytes([s[2 * i], s[2 * i + 1]]) as u32,
        }
    }
}

/// Resource limits for one run.
#[derive(Clone, Copy, Debug)]
pub struct ExecLimits {
    /// Maximum backtrack stack size in `u32` words.
    pub max_stack: usize,
}

impl Default for ExecLimits {
    fn default() -> ExecLimits {
        ExecLimits { max_stack: 1 << 20 }
    }
}

/// Capture positions of a match, as UTF-16 (or Latin-1) indices: a view of the state's
/// slots ([`ExecState::captures`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Caps<'a> {
    slots: &'a [u32],
}

impl Caps<'_> {
    /// Number of groups, including group 0.
    pub fn len(&self) -> usize {
        self.slots.len() / 2
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The `(start, end)` of group `i`, or `None` if it did not participate.
    pub fn get(&self, i: usize) -> Option<(usize, usize)> {
        let (s, e) = (self.slots[2 * i], self.slots[2 * i + 1]);
        (s != UNSET && e != UNSET).then_some((s as usize, e as usize))
    }
}

/// Owned capture positions ([`exec`]'s result).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Captures {
    slots: Vec<u32>,
}

impl Captures {
    pub fn caps(&self) -> Caps<'_> {
        Caps { slots: &self.slots }
    }

    pub fn len(&self) -> usize {
        self.caps().len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<(usize, usize)> {
        self.caps().get(i)
    }
}

/// How a run of [`exec_in`] or [`resume_in`] ended; the captures of a match are in the
/// state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Match,
    NoMatch,
    /// The fuel ran out; [`resume_in`] continues (with the same program and input).
    OutOfFuel,
    /// The backtrack stack would exceed [`ExecLimits::max_stack`].
    StackOverflow,
}

pub enum ExecResult {
    Match(Captures),
    NoMatch,
    /// The fuel ran out; pass the state to [`resume`] (with the same program and input).
    OutOfFuel(ExecState),
    /// The backtrack stack would exceed [`ExecLimits::max_stack`].
    StackOverflow,
}

/// A match in progress, or the captures of the last one. One state can run match after
/// match (a global replace keeps one), reusing its memory.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ExecState {
    pc: u32,
    pos: u32,
    /// The top frame: the index in `mem` just above it, `2 * nslots` with no frame.
    bp: u32,
    next_gen: u32,
    /// Where the current attempt started.
    start: u32,
    /// Whether a failed attempt moves on to the next start (a search).
    search: bool,
    nslots: u32,
    /// The slots, their generations (`slot_gen`), then the backtrack stack.
    mem: Vec<u32>,
}

impl ExecState {
    pub fn new() -> ExecState {
        ExecState::default()
    }

    /// The words of memory the state holds on to (its slots and backtrack stack).
    pub fn capacity(&self) -> usize {
        self.mem.capacity()
    }

    /// The captures of the match just found.
    pub fn captures(&self, prog: Prog<'_>) -> Caps<'_> {
        Caps {
            slots: &self.mem[..2 * prog.capture_count()],
        }
    }

    /// The state as plain words, for storing it in the arena.
    pub fn to_words(&self) -> Vec<u32> {
        let mut w = alloc::vec![
            self.pc,
            self.pos,
            self.bp,
            self.next_gen,
            self.start,
            self.search as u32,
            self.nslots
        ];
        w.extend_from_slice(&self.mem);
        w
    }

    /// Rebuilds a state from [`ExecState::to_words`].
    pub fn from_words(w: &[u32]) -> Option<ExecState> {
        let nslots = *w.get(6)?;
        let mem = w[7..].to_vec();
        if mem.len() < 2 * nslots as usize || (w[2] as usize) < 2 * nslots as usize {
            return None;
        }
        Some(ExecState {
            pc: w[0],
            pos: w[1],
            bp: w[2],
            next_gen: w[3],
            start: w[4],
            search: w[5] != 0,
            nslots,
            mem,
        })
    }
}

/// The program's words as byte quads (for [`exec`], which takes a [`Program`]).
fn quads(prog: &Program) -> Vec<[u8; 4]> {
    prog.words().iter().map(|w| w.to_le_bytes()).collect()
}

/// Runs `prog` on `input` starting at `start` (a UTF-16 index). Unless the program is
/// sticky, the match is searched for from `start` on. Each instruction costs one unit of
/// `fuel` (a backreference costs its length, and may take `fuel` below zero: the caller
/// carries the overdraft). The match stops with [`ExecResult::OutOfFuel`] once `fuel` is
/// not positive.
pub fn exec(
    prog: &Program,
    input: Input<'_>,
    start: usize,
    fuel: &mut i64,
    limits: ExecLimits,
) -> ExecResult {
    let sticky = prog.flags().contains(super::Flags::STICKY);
    exec_at(prog, input, start, sticky, fuel, limits)
}

/// [`exec`] with stickiness chosen by the caller.
pub fn exec_at(
    prog: &Program,
    input: Input<'_>,
    start: usize,
    sticky: bool,
    fuel: &mut i64,
    limits: ExecLimits,
) -> ExecResult {
    let q = quads(prog);
    let mut st = ExecState::new();
    let o = exec_in(Prog(&q), input, start, sticky, fuel, limits, &mut st);
    result(Prog(&q), o, st)
}

/// Continues a match suspended by running out of fuel.
pub fn resume(
    prog: &Program,
    input: Input<'_>,
    mut state: ExecState,
    fuel: &mut i64,
    limits: ExecLimits,
) -> ExecResult {
    let q = quads(prog);
    let o = resume_in(Prog(&q), input, fuel, limits, &mut state);
    result(Prog(&q), o, state)
}

fn result(prog: Prog<'_>, o: Outcome, st: ExecState) -> ExecResult {
    match o {
        Outcome::Match => ExecResult::Match(Captures {
            slots: st.captures(prog).slots.to_vec(),
        }),
        Outcome::NoMatch => ExecResult::NoMatch,
        Outcome::OutOfFuel => ExecResult::OutOfFuel(st),
        Outcome::StackOverflow => ExecResult::StackOverflow,
    }
}

/// [`exec_at`] on a program read in place, in the state `st` (whatever it held before):
/// `sticky` false searches even for a sticky program (RegExp.prototype[@@split] on an
/// unmodified RegExp, D1503).
pub fn exec_in(
    prog: Prog<'_>,
    input: Input<'_>,
    start: usize,
    sticky: bool,
    fuel: &mut i64,
    limits: ExecLimits,
    st: &mut ExecState,
) -> Outcome {
    if start > input.len() {
        return Outcome::NoMatch;
    }
    let unicode = prog.flags().unicode_mode();
    let mut start = start;
    if unicode && start > 0 && start < input.len() {
        // Starting inside a surrogate pair starts at the pair (RegExpBuiltinExec).
        if (0xDC00..=0xDFFF).contains(&input.unit(start))
            && (0xD800..=0xDBFF).contains(&input.unit(start - 1))
        {
            start -= 1;
        }
    }
    let nslots = 2 * prog.at(H_CAPTURES) + prog.at(H_REGISTERS);
    // A pattern anchored at the input's start can only match where it starts.
    let search = !sticky && prog.at(H_MODE) & M_ANCHOR != A_INPUT;
    st.pc = if search { PC_FIND } else { prog.at(H_ANCHORED) };
    st.pos = start as u32;
    st.start = start as u32;
    st.bp = 2 * nslots;
    st.next_gen = 0;
    st.search = search;
    st.nslots = nslots;
    st.mem.clear();
    // Most matches need a few frames; growing from empty reallocates repeatedly.
    st.mem.reserve(2 * nslots as usize + 64);
    st.mem.resize(2 * nslots as usize, UNSET);
    resume_in(prog, input, fuel, limits, st)
}

/// Continues a match in `st` that ran out of fuel.
pub fn resume_in(
    prog: Prog<'_>,
    input: Input<'_>,
    fuel: &mut i64,
    limits: ExecLimits,
    st: &mut ExecState,
) -> Outcome {
    let mode = prog.at(H_MODE);
    let mut m = Machine {
        code: prog,
        input,
        unicode: prog.flags().unicode_mode(),
        regs: 2 * prog.at(H_CAPTURES) as usize,
        nslots: st.nslots as usize,
        base: 2 * st.nslots as usize,
        mode,
        limits,
        s: st,
    };
    m.run(fuel)
}

struct Machine<'a, 's> {
    code: Prog<'a>,
    input: Input<'a>,
    unicode: bool,
    /// Index of the first register slot.
    regs: usize,
    /// Index of the first slot generation.
    nslots: usize,
    /// Where the backtrack stack starts in `mem`.
    base: usize,
    /// The program's `H_MODE` word.
    mode: u32,
    limits: ExecLimits,
    s: &'s mut ExecState,
}

/// Why an instruction did not continue normally.
enum Stop {
    Fail,
    Overflow,
}

/// What looking for the next start of a search found.
enum Find {
    Start,
    None,
    OutOfFuel,
}

impl Machine<'_, '_> {
    fn next_char(&self, pos: usize) -> Option<(u32, usize)> {
        if pos >= self.input.len() {
            return None;
        }
        let c = self.input.unit(pos);
        if self.unicode && (0xD800..=0xDBFF).contains(&c) && pos + 1 < self.input.len() {
            let d = self.input.unit(pos + 1);
            if (0xDC00..=0xDFFF).contains(&d) {
                return Some((0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00), pos + 2));
            }
        }
        Some((c, pos + 1))
    }

    fn prev_char(&self, pos: usize) -> Option<(u32, usize)> {
        if pos == 0 {
            return None;
        }
        let c = self.input.unit(pos - 1);
        if self.unicode && (0xDC00..=0xDFFF).contains(&c) && pos >= 2 {
            let h = self.input.unit(pos - 2);
            if (0xD800..=0xDBFF).contains(&h) {
                return Some((0x10000 + ((h - 0xD800) << 10) + (c - 0xDC00), pos - 2));
            }
        }
        Some((c, pos - 1))
    }

    /// Reads one character forward or backward from the current position and advances.
    fn read(&mut self, back: bool) -> Option<u32> {
        let pos = self.s.pos as usize;
        let (c, p) = if back {
            self.prev_char(pos)?
        } else {
            self.next_char(pos)?
        };
        self.s.pos = p as u32;
        Some(c)
    }

    fn push(&mut self, words: &[u32]) -> Result<(), Stop> {
        if self.s.mem.len() - self.base + words.len() > self.limits.max_stack {
            return Err(Stop::Overflow);
        }
        self.s.mem.extend_from_slice(words);
        Ok(())
    }

    fn push_frame(&mut self, pc: u32, kind: u32) -> Result<(), Stop> {
        let g = self.s.next_gen;
        self.s.next_gen = g.wrapping_add(1);
        self.push(&[pc, self.s.pos, self.s.bp << 2 | kind, g])?;
        self.s.bp = self.s.mem.len() as u32;
        Ok(())
    }

    fn write_slot(&mut self, slot: usize, value: u32) -> Result<(), Stop> {
        let bp = self.s.bp as usize;
        if bp > self.base {
            let g = self.s.mem[bp - 1];
            let gen_at = self.nslots + slot;
            if self.s.mem[gen_at] != g {
                let old = self.s.mem[slot];
                let old_gen = self.s.mem[gen_at];
                self.push(&[slot as u32, old, old_gen])?;
                self.s.mem[gen_at] = g;
            }
        }
        self.s.mem[slot] = value;
        Ok(())
    }

    /// Undoes the slot writes logged above the current frame.
    fn undo_to_frame(&mut self) {
        let bp = self.s.bp as usize;
        let mem = &mut self.s.mem;
        while mem.len() > bp {
            let n = mem.len();
            let (slot, old, old_gen) = (mem[n - 3] as usize, mem[n - 2], mem[n - 1]);
            mem[slot] = old;
            mem[self.nslots + slot] = old_gen;
            mem.truncate(n - 3);
        }
    }

    /// Pops the top frame (after undoing its log); returns its kind.
    fn pop_frame(&mut self) -> u32 {
        self.undo_to_frame();
        let f = self.s.bp as usize - FRAME;
        let st = &self.s.mem;
        self.s.pc = st[f];
        self.s.pos = st[f + 1];
        let kind = st[f + 2] & 3;
        self.s.bp = st[f + 2] >> 2;
        self.s.mem.truncate(f);
        kind
    }

    /// Backtracks to the most recent alternative. Returns false if there is none.
    fn backtrack(&mut self) -> bool {
        loop {
            if self.s.bp as usize == self.base {
                return false;
            }
            // A failed positive lookahead keeps failing; a failed negative one succeeds
            // (its frame's pc is the continuation).
            match self.pop_frame() {
                K_LOOK => {}
                K_STAR => {
                    if self.star_retry() {
                        return true;
                    }
                }
                _ => return true,
            }
        }
    }

    /// After a failed attempt of a search: the next start is one character on, or past
    /// the run of the leading `STAR` (every start inside it fails too, D1501). False if
    /// there is none.
    fn next_attempt(&mut self) -> bool {
        if !self.s.search {
            return false;
        }
        let mut from = self.s.start as usize;
        if self.mode & M_SKIP != 0 {
            let to = self.s.mem[self.regs + self.code.at(H_SKIP) as usize];
            if to != UNSET && to as usize > from {
                from = to as usize;
            }
        }
        let Some((_, next)) = self.next_char(from) else {
            return false;
        };
        // The attempt left its captures behind (writes without a frame are not logged).
        self.s.mem[..self.regs].fill(UNSET);
        self.s.pos = next as u32;
        self.s.pc = PC_FIND;
        true
    }

    /// Moves `pos` to the next position where an attempt can match: the start of a line
    /// for a pattern that starts with a multiline `^`, and a character of the program's
    /// first set if it has one. One unit of fuel per character passed (`find_line`: per
    /// code unit).
    fn find(&mut self, fuel: &mut i64) -> Find {
        let first = self.code.at(H_SEARCH) as usize;
        if self.mode & M_ANCHOR == A_LINE {
            return self.find_line(first, fuel);
        }
        if first == 0 {
            return Find::Start;
        }
        if !self.unicode {
            return self.find_first(first, fuel);
        }
        loop {
            let p = self.s.pos as usize;
            let next = self.next_char(p);
            if let Some((c, _)) = next
                && self.in_first(first, c)
            {
                return Find::Start;
            }
            let Some((_, np)) = next else {
                return Find::None;
            };
            // Paid before moving on, so a resumed search examines `p` again and the
            // total does not depend on where the fuel ran out (D1711).
            if *fuel <= 0 {
                return Find::OutOfFuel;
            }
            *fuel -= 1;
            self.s.pos = np as u32;
        }
    }

    /// `find` for a pattern anchored at line starts: from a position that cannot start
    /// a match, everything up to and including the next line terminator is passed in one
    /// loop (one unit of fuel per code unit).
    fn find_line(&mut self, first: usize, fuel: &mut i64) -> Find {
        let len = self.input.len();
        loop {
            let p = self.s.pos as usize;
            if (p == 0 || is_line_terminator(self.input.unit(p - 1)))
                && (first == 0
                    || self
                        .next_char(p)
                        .is_some_and(|(c, _)| self.in_first(first, c)))
            {
                return Find::Start;
            }
            if p >= len {
                return Find::None;
            }
            let passes = usize::try_from((*fuel).max(0)).unwrap_or(usize::MAX);
            let limit = len.min(p.saturating_add(passes));
            match (p..limit).find(|&i| is_line_terminator(self.input.unit(i))) {
                Some(q) => {
                    *fuel -= (q + 1 - p) as i64;
                    self.s.pos = (q + 1) as u32;
                }
                None if limit == len => {
                    *fuel -= (len - p) as i64;
                    self.s.pos = len as u32;
                    return Find::None;
                }
                None => {
                    *fuel -= passes as i64;
                    self.s.pos = limit as u32;
                    return Find::OutOfFuel;
                }
            }
        }
    }

    /// Whether `c` is in the first set at `first` (`n (lo hi){n}` then a bitmap of the
    /// Latin-1 characters in 8 words).
    #[inline(always)]
    fn in_first(&self, first: usize, c: u32) -> bool {
        let n = self.code.at(first) as usize;
        if c < 256 {
            self.code.at(first + 1 + 2 * n + (c >> 5) as usize) >> (c & 31) & 1 != 0
        } else {
            in_ranges(self.code.sub(first + 1, 2 * n), c)
        }
    }

    /// `find` for code units (no `u`/`v`) and no line anchor: a tight loop per input
    /// representation.
    fn find_first(&mut self, first: usize, fuel: &mut i64) -> Find {
        let p = self.s.pos as usize;
        // The same accounting as `find`: examining a position is free, passing it costs
        // one unit, so at most fuel + 1 positions are examined.
        let passes = usize::try_from((*fuel).max(0)).unwrap_or(usize::MAX);
        let len = self.input.len();
        let end = len.min(p.saturating_add(passes).saturating_add(1));
        // One-byte strings are the common input: their loop on its own.
        let found = match self.input {
            Input::Latin1(b) => (p..end).find(|&i| self.in_first(first, b[i] as u32)),
            input => (p..end).find(|&i| self.in_first(first, input.unit(i))),
        };
        match found {
            Some(i) => {
                *fuel -= (i - p) as i64;
                self.s.pos = i as u32;
                Find::Start
            }
            None if len - p <= passes => {
                *fuel -= (len - p) as i64;
                self.s.pos = len as u32;
                Find::None
            }
            None => {
                // The last position examined is passed by the next slice.
                *fuel -= passes as i64;
                self.s.pos = (p + passes) as u32;
                Find::OutOfFuel
            }
        }
    }

    /// The character matcher at `pc` (`CHAR`, `CLASS`, `DOT` or `ANY`) at the current
    /// position: the position after the character if it matches.
    #[inline]
    fn single(&self, pc: usize, back: bool) -> Option<u32> {
        let word = self.code.at(pc);
        let pos = self.s.pos as usize;
        let (mut c, next) = if back {
            self.prev_char(pos)?
        } else {
            self.next_char(pos)?
        };
        if word & F_ICASE != 0 {
            c = canonicalize(c, self.unicode);
        }
        let ok = match word & OP_MASK {
            CHAR => c == self.code.at(pc + 1),
            CLASS => {
                let n = self.code.at(pc + 1) as usize;
                in_ranges(self.code.sub(pc + 2, 2 * n), c)
            }
            DOT => !is_line_terminator(c),
            _ => true,
        };
        ok.then_some(next as u32)
    }

    /// Matches up to `cap` characters forward with the case-sensitive one-character
    /// matcher at `pc` (D1562): the count and the position after them.
    fn scan_forward(&self, pc: usize, cap: u32) -> (u32, u32) {
        match self.input {
            Input::Latin1(b) => self.scan_with(pc, cap, b.len(), |i| b[i] as u32),
            Input::Utf16(u) => self.scan_with(pc, cap, u.len(), |i| u[i] as u32),
            Input::Utf16Le(b) => self.scan_with(pc, cap, b.len() / 2, |i| {
                u16::from_le_bytes([b[2 * i], b[2 * i + 1]]) as u32
            }),
        }
    }

    #[inline(always)]
    fn scan_with(
        &self,
        pc: usize,
        cap: u32,
        len: usize,
        unit: impl Fn(usize) -> u32,
    ) -> (u32, u32) {
        let code = self.code;
        let op = code.at(pc) & OP_MASK;
        let ranges = if op == CLASS {
            let n = code.at(pc + 1) as usize;
            code.sub(pc + 2, 2 * n)
        } else {
            code.sub(pc, 0)
        };
        let nr = ranges.0.len();
        let ch = code.at(pc + 1);
        let mut hint = 0usize;
        let mut pos = self.s.pos as usize;
        let mut count = 0;
        while count < cap && pos < len {
            let mut c = unit(pos);
            let mut next = pos + 1;
            if self.unicode && (0xD800..0xDC00).contains(&c) && next < len {
                let d = unit(next);
                if (0xDC00..0xE000).contains(&d) {
                    c = 0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00);
                    next += 1;
                }
            }
            let ok = match op {
                CHAR => c == ch,
                CLASS => {
                    // Consecutive characters usually fall in the same range.
                    if hint < nr && ranges.at(hint) <= c && c <= ranges.at(hint + 1) {
                        true
                    } else {
                        let (mut lo, mut hi) = (0, nr / 2);
                        let mut found = false;
                        while lo < hi {
                            let mid = (lo + hi) / 2;
                            if c < ranges.at(2 * mid) {
                                hi = mid;
                            } else if c > ranges.at(2 * mid + 1) {
                                lo = mid + 1;
                            } else {
                                hint = 2 * mid;
                                found = true;
                                break;
                            }
                        }
                        found
                    }
                }
                DOT => !is_line_terminator(c),
                _ => true,
            };
            if !ok {
                break;
            }
            pos = next;
            count += 1;
        }
        (count, pos as u32)
    }

    /// One position back towards the loop's start `start` (the other way for a backward
    /// loop): a whole code point, unless that would cross `start`.
    fn give_back(&self, back: bool, start: u32) -> u32 {
        let pos = self.s.pos as usize;
        if back {
            match self.next_char(pos) {
                Some((_, p)) if p as u32 <= start => p as u32,
                _ => pos as u32 + 1,
            }
        } else {
            match self.prev_char(pos) {
                Some((_, p)) if p as u32 >= start => p as u32,
                _ => pos as u32 - 1,
            }
        }
    }

    /// Whether the character at the current position can start the continuation of the
    /// `STAR` at `pc` (always, without a follow set).
    fn star_follows(&self, pc: usize) -> bool {
        let code = self.code;
        if code.at(pc) & F_CHECK == 0 {
            return true;
        }
        let n = code.at(pc + 5) as usize;
        let at = pc + 6 + code.at(pc + 4) as usize;
        match self.next_char(self.s.pos as usize) {
            Some((c, _)) => in_ranges(code.sub(at, 2 * n), c),
            None => code.at(pc) & F_END != 0,
        }
    }

    /// Moves a `STAR` loop with `n` iterations to the next count whose position can start
    /// the continuation: giving back characters (greedy) or taking more (lazy). Writes the
    /// count register if it changed; `None` if there is no such count.
    fn star_settle(&mut self, pc: usize, mut n: u32) -> Option<u32> {
        let code = self.code;
        let word = code.at(pc);
        let r = self.regs + code.at(pc + 1) as usize;
        let (min, max) = (code.at(pc + 2), code.at(pc + 3));
        let back = word & F_BACK != 0;
        let n0 = n;
        while !self.star_follows(pc) {
            if word & F_GREEDY != 0 {
                if n <= min || word & F_DISJOINT != 0 {
                    return None;
                }
                self.s.pos = self.give_back(back, self.s.mem[r + 1]);
                n -= 1;
            } else {
                if n >= max {
                    return None;
                }
                self.s.pos = self.single(pc + 6, back)?;
                n += 1;
            }
        }
        if n != n0 && self.write_slot(r, n).is_err() {
            return None;
        }
        Some(n)
    }

    /// Backtracking into a `STAR` frame (D1501): greedy gives back one character, lazy
    /// takes one more; then the continuation runs again with a frame for the next retry.
    /// False if the loop has no choices left.
    fn star_retry(&mut self) -> bool {
        let pc = self.s.pc as usize;
        let word = self.code.at(pc);
        let r = self.regs + self.code.at(pc + 1) as usize;
        let (min, max) = (self.code.at(pc + 2), self.code.at(pc + 3));
        let next = star_next(self.code, pc);
        let back = word & F_BACK != 0;
        let n = self.s.mem[r];
        let n = if word & F_GREEDY != 0 {
            if n <= min {
                return false;
            }
            self.s.pos = self.give_back(back, self.s.mem[r + 1]);
            n - 1
        } else {
            if n >= max {
                return false;
            }
            match self.single(pc + 6, back) {
                Some(p) => self.s.pos = p,
                None => return false,
            }
            n + 1
        };
        // Retries that cannot work are skipped without a frame.
        let Some(n) = self.star_settle(pc, n) else {
            return false;
        };
        // Logged under the frame below, so a later retry sees this count. Overflow is
        // impossible here: the frame just popped freed more than this needs.
        if self.write_slot(r, n).is_err() {
            return false;
        }
        let more = if word & F_GREEDY != 0 {
            n > min
        } else {
            n < max
        };
        if more && self.push_frame(pc as u32, K_STAR).is_err() {
            return false;
        }
        self.s.pc = next;
        true
    }

    /// The body of a positive lookaround matched: drop the frames back to and including the
    /// lookaround's own, keeping their slot writes (moved under the frame below).
    fn look_succeeded(&mut self) {
        let mut tops = Vec::new(); // frame start indices, top first
        let mut bp = self.s.bp as usize;
        let look_start;
        loop {
            let f = bp - FRAME;
            let w = self.s.mem[f + 2];
            if w & 3 == K_LOOK {
                look_start = f;
                bp = (w >> 2) as usize;
                break;
            }
            tops.push_out(f);
            bp = (w >> 2) as usize;
        }
        // A lookaround does not consume input.
        self.s.pos = self.s.mem[look_start + 1];
        let st = &mut self.s.mem;
        let mut out = look_start;
        if bp > self.base {
            // Copy the logs: [look_start + FRAME, next frame), ... , [last frame + FRAME, len).
            let mut from = look_start + FRAME;
            let len = st.len();
            for &f in tops.iter().rev().chain(core::iter::once(&len)) {
                while from < f {
                    st[out] = st[from];
                    out += 1;
                    from += 1;
                }
                from = f + FRAME;
            }
        }
        st.truncate(out);
        self.s.bp = bp as u32;
    }

    /// The body of a negative lookaround matched: undo everything back to its frame, which
    /// is then dropped.
    fn neg_look_matched(&mut self) {
        while self.pop_frame() != K_NEG_LOOK {}
    }

    fn run(&mut self, fuel: &mut i64) -> Outcome {
        loop {
            if *fuel <= 0 {
                return Outcome::OutOfFuel;
            }
            if self.s.pc == PC_FIND {
                match self.find(fuel) {
                    Find::Start => {
                        self.s.start = self.s.pos;
                        self.s.pc = self.code.at(H_ANCHORED);
                    }
                    Find::None => return Outcome::NoMatch,
                    Find::OutOfFuel => return Outcome::OutOfFuel,
                }
                continue;
            }
            *fuel -= 1;
            match self.step(fuel) {
                Ok(true) => return Outcome::Match,
                Ok(false) => {}
                Err(Stop::Fail) => {
                    if !self.backtrack() && !self.next_attempt() {
                        return Outcome::NoMatch;
                    }
                }
                Err(Stop::Overflow) => return Outcome::StackOverflow,
            }
        }
    }

    /// Executes one instruction. `Ok(true)` is a match.
    fn step(&mut self, fuel: &mut i64) -> Result<bool, Stop> {
        let code = self.code;
        let pc = self.s.pc as usize;
        let word = code.at(pc);
        let op = word & OP_MASK;
        let back = word & F_BACK != 0;
        let icase = word & F_ICASE != 0;
        let fail = Err(Stop::Fail);
        match op {
            MATCH => return Ok(true),
            // A run of literal characters (a word of the pattern) in one step. Each is
            // an instruction of its own for the fuel: it costs its unit, and the run
            // stops where the fuel does.
            CHAR if word == CHAR => {
                let mut pc = pc;
                loop {
                    if self.read(false) != Some(code.at(pc + 1)) {
                        return fail;
                    }
                    pc += 2;
                    self.s.pc = pc as u32;
                    if *fuel <= 0 || code.at(pc) != CHAR {
                        break;
                    }
                    *fuel -= 1;
                }
            }
            CHAR => {
                let Some(mut c) = self.read(back) else {
                    return fail;
                };
                if icase {
                    c = canonicalize(c, self.unicode);
                }
                if c != code.at(pc + 1) {
                    return fail;
                }
                self.s.pc += 2;
            }
            CLASS => {
                let n = code.at(pc + 1) as usize;
                let Some(mut c) = self.read(back) else {
                    return fail;
                };
                if icase {
                    c = canonicalize(c, self.unicode);
                }
                if !in_ranges(code.sub(pc + 2, 2 * n), c) {
                    return fail;
                }
                self.s.pc += 2 + 2 * n as u32;
            }
            DOT => {
                match self.read(back) {
                    Some(c) if !is_line_terminator(c) => {}
                    _ => return fail,
                }
                self.s.pc += 1;
            }
            ANY => {
                if self.read(back).is_none() {
                    return fail;
                }
                self.s.pc += 1;
            }
            LINE_START => {
                let pos = self.s.pos as usize;
                if pos > 0 {
                    let ok = word & F_MULTILINE != 0
                        && self
                            .prev_char(pos)
                            .is_some_and(|(c, _)| is_line_terminator(c));
                    if !ok {
                        return fail;
                    }
                }
                self.s.pc += 1;
            }
            LINE_END => {
                let pos = self.s.pos as usize;
                if pos < self.input.len() {
                    let ok = word & F_MULTILINE != 0
                        && self
                            .next_char(pos)
                            .is_some_and(|(c, _)| is_line_terminator(c));
                    if !ok {
                        return fail;
                    }
                }
                self.s.pc += 1;
            }
            WORD_BOUNDARY => {
                let pos = self.s.pos as usize;
                let ui = word & F_UICASE != 0;
                let a = self
                    .prev_char(pos)
                    .is_some_and(|(c, _)| is_word_char(c, ui));
                let b = self
                    .next_char(pos)
                    .is_some_and(|(c, _)| is_word_char(c, ui));
                if (a != b) == (word & F_NEG != 0) {
                    return fail;
                }
                self.s.pc += 1;
            }
            STAR => {
                let r = self.regs + code.at(pc + 1) as usize;
                let (min, max) = (code.at(pc + 2), code.at(pc + 3));
                let greedy = word & F_GREEDY != 0;
                // Greedy matches all it can, lazy only the minimum. Resumable: the count
                // is in a register (written when the loop stops) and the position in the
                // state.
                let limit = if greedy { max } else { min };
                let start = self.s.mem[r];
                let mut n = start;
                let mut stopped = false;
                let fast = !back && code.at(pc + 6) & F_ICASE == 0;
                if fast && n < limit {
                    // The common case in one tight loop (D1562); same fuel accounting as
                    // the general loop below: one unit per character after the first.
                    let cap = (limit - n).min(
                        u32::try_from((*fuel).max(0))
                            .unwrap_or(u32::MAX)
                            .saturating_add(1),
                    );
                    let (m, pos) = self.scan_forward(pc + 6, cap);
                    self.s.pos = pos;
                    n += m;
                    if m > 0 && i64::from(m) == (*fuel).max(0).saturating_add(1) {
                        stopped = true;
                        *fuel = 0;
                    } else {
                        *fuel -= i64::from(m);
                    }
                }
                while !fast && n < limit {
                    match self.single(pc + 6, back) {
                        Some(p) => {
                            self.s.pos = p;
                            n += 1;
                        }
                        None => break,
                    }
                    if *fuel <= 0 {
                        stopped = true;
                        break;
                    }
                    *fuel -= 1;
                }
                if n != start {
                    self.write_slot(r, n)?;
                }
                if stopped {
                    return Ok(false);
                }
                if word & F_LEAD != 0 {
                    // Not logged: the search reads it after this attempt failed.
                    let at = star_next(code, pc) as usize - 1;
                    self.s.mem[self.regs + code.at(at) as usize] = self.s.pos;
                }
                if n < min {
                    return fail;
                }
                let Some(n) = self.star_settle(pc, n) else {
                    return fail;
                };
                let more = word & F_LAST == 0
                    && if greedy {
                        n > min && word & F_DISJOINT == 0
                    } else {
                        n < max
                    };
                if more {
                    self.push_frame(pc as u32, K_STAR)?;
                }
                self.s.pc = star_next(code, pc);
            }
            GOTO => self.s.pc = code.at(pc + 1),
            SPLIT_NEXT => {
                self.push_frame(code.at(pc + 1), K_SPLIT)?;
                self.s.pc += 2;
            }
            SPLIT_GOTO => {
                self.push_frame(pc as u32 + 2, K_SPLIT)?;
                self.s.pc = code.at(pc + 1);
            }
            SAVE => {
                self.write_slot(code.at(pc + 1) as usize, self.s.pos)?;
                self.s.pc += 2;
            }
            RESET => {
                for g in code.at(pc + 1)..=code.at(pc + 2) {
                    self.write_slot(2 * g as usize, UNSET)?;
                    self.write_slot(2 * g as usize + 1, UNSET)?;
                }
                self.s.pc += 3;
            }
            BACKREF => {
                let n = code.at(pc + 1) as usize;
                let span = (0..n).find_map(|i| {
                    let g = code.at(pc + 2 + i) as usize;
                    let (s, e) = (self.s.mem[2 * g], self.s.mem[2 * g + 1]);
                    (s != UNSET && e != UNSET).then_some((s as usize, e as usize))
                });
                if let Some((s, e)) = span {
                    // Charged in full, even past the end of the fuel (a backreference
                    // cannot stop half-way): the overdraft is the caller's debt, so the
                    // total does not depend on how much fuel a slice had left (D1711).
                    *fuel -= (e - s) as i64;
                    if !self.match_backref(s, e, back, icase) {
                        return fail;
                    }
                }
                self.s.pc += 2 + n as u32;
            }
            LOOK => {
                let kind = if word & F_NEG != 0 {
                    K_NEG_LOOK
                } else {
                    K_LOOK
                };
                self.push_frame(code.at(pc + 1), kind)?;
                self.s.pc += 2;
            }
            LOOK_END => {
                if word & F_NEG != 0 {
                    self.neg_look_matched();
                    return fail;
                }
                self.look_succeeded();
                self.s.pc += 1;
            }
            SET_REG => {
                self.write_slot(self.regs + code.at(pc + 1) as usize, code.at(pc + 2))?;
                self.s.pc += 3;
            }
            SET_POS => {
                self.write_slot(self.regs + code.at(pc + 1) as usize, self.s.pos)?;
                self.s.pc += 2;
            }
            CHECK_ADV => {
                if self.s.mem[self.regs + code.at(pc + 1) as usize] == self.s.pos {
                    return fail;
                }
                self.s.pc += 2;
            }
            LOOP => {
                let r = self.regs + code.at(pc + 1) as usize;
                let v = self.s.mem[r].wrapping_sub(1);
                self.write_slot(r, v)?;
                self.s.pc = if v != 0 {
                    code.at(pc + 2)
                } else {
                    pc as u32 + 3
                };
            }
            LOOP_SPLIT => {
                let r = self.regs + code.at(pc + 1) as usize;
                let limit = code.at(pc + 2);
                let target = code.at(pc + 3);
                let next = pc as u32 + 4;
                let v = self.s.mem[r].wrapping_sub(1);
                self.write_slot(r, v)?;
                if v > limit {
                    // Still below the minimum: iterate.
                    self.s.pc = target;
                } else {
                    if word & F_CHECK != 0 && self.s.mem[r + 1] == self.s.pos && v != limit {
                        // An optional iteration that matched empty.
                        return fail;
                    }
                    if v == 0 {
                        self.s.pc = next;
                    } else if word & F_GREEDY != 0 {
                        self.push_frame(next, K_SPLIT)?;
                        self.s.pc = target;
                    } else {
                        self.push_frame(target, K_SPLIT)?;
                        self.s.pc = next;
                    }
                }
            }
            _ => unreachable!("bad regexp opcode {op}"),
        }
        Ok(false)
    }

    /// Matches the text `[s, e)` at the current position (backward: ending there).
    fn match_backref(&mut self, s: usize, e: usize, back: bool, icase: bool) -> bool {
        let canon = |c: u32| {
            if icase {
                canonicalize(c, self.unicode)
            } else {
                c
            }
        };
        if !back {
            let mut p = s;
            let mut pos = self.s.pos as usize;
            while p < e {
                // Characters of the capture, limited to the capture's end.
                let (c1, np) = match self.next_char(p) {
                    Some((c, np)) if np <= e => (c, np),
                    _ => (self.input.unit(p), p + 1),
                };
                let Some((c2, npos)) = self.next_char(pos) else {
                    return false;
                };
                if canon(c1) != canon(c2) {
                    return false;
                }
                p = np;
                pos = npos;
            }
            self.s.pos = pos as u32;
        } else {
            let mut p = e;
            let mut pos = self.s.pos as usize;
            while p > s {
                let (c1, np) = match self.prev_char(p) {
                    Some((c, np)) if np >= s => (c, np),
                    _ => (self.input.unit(p - 1), p - 1),
                };
                let Some((c2, npos)) = self.prev_char(pos) else {
                    return false;
                };
                if canon(c1) != canon(c2) {
                    return false;
                }
                p = np;
                pos = npos;
            }
            self.s.pos = pos as u32;
        }
        true
    }
}
