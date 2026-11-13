use std::ops::Range;

use super::program::{BYTE_VALUES, Instruction, LoopFields, Program, Start};
use super::set::{ASCII_LAST, CodePointSet};
use super::unicode;

#[derive(Clone, Copy, Debug)]
struct LoopState {
    iterations: usize,
    entry: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct GroupState {
    start: Option<usize>,
    end: Option<usize>,
}

impl GroupState {
    fn range(self) -> Option<Range<usize>> {
        Some(self.start?..self.end?)
    }
}

#[derive(Clone, Copy, Debug)]
enum Backtrack {
    SetPosition {
        ip: usize,
        position: usize,
    },
    SetLoop {
        id: usize,
        state: LoopState,
    },
    SetGroup {
        id: usize,
        state: GroupState,
    },
    EnterLazyLoop {
        ip: usize,
        entry_before: usize,
        state: LoopState,
    },
    GreedyOneCharacter {
        continuation: usize,
        min: usize,
        max: usize,
    },
    LazyOneCharacter {
        continuation: usize,
        min: usize,
        max: usize,
    },
}

enum Flow {
    Continue,
    Goal,
    Backtrack,
}

pub(super) fn is_match(program: &Program, text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut machine = Machine {
        program,
        text,
        loops: Vec::new(),
        groups: Vec::new(),
        stack: Vec::new(),
        saved_groups: Vec::new(),
    };
    let mut required = match &program.required {
        Some(literal) => match literal.find(bytes) {
            Some(at) => Some((literal, at)),
            None => return false,
        },
        None => None,
    };
    let mut prepared = false;
    let mut start = 0;
    loop {
        let rest = &bytes[start..];
        let candidate = match &program.start {
            Start::Literal(literal) => literal.find(rest),
            Start::TwoBytes(first, second) => memchr::memchr2(*first, *second, rest),
            Start::ThreeBytes(first, second, third) => {
                memchr::memchr3(*first, *second, *third, rest)
            }
            Start::Bytes(set) => find_in_set(rest, set),
            Start::Anywhere | Start::Anchored => Some(0),
        };
        let Some(offset) = candidate else {
            return false;
        };
        start += offset;
        if let Some((literal, at)) = &mut required
            && *at < start
        {
            let Some(found) = literal.find(&bytes[start..]) else {
                return false;
            };
            *at = start + found;
        }
        if !lead_matches(&program.lead, &bytes[start..]) {
            if matches!(program.start, Start::Anchored) {
                return false;
            }
            let Some(next) = machine.right_boundary(start) else {
                return false;
            };
            start = next;
            continue;
        }
        if !prepared {
            prepared = true;
            machine.loops = vec![
                LoopState {
                    iterations: 0,
                    entry: 0,
                };
                program.loop_count
            ];
            machine.groups = vec![GroupState::default(); program.group_count];
        }
        if machine.attempt(0, start, true).is_some() {
            return true;
        }
        if matches!(program.start, Start::Anchored) {
            return false;
        }
        if let Some(ip) = program.leading_run {
            let matcher = &program.instructions[ip + 1];
            start = machine.repeat_one(matcher, start, usize::MAX, true).0;
        }
        let Some(next) = machine.right_boundary(start) else {
            return false;
        };
        start = next;
    }
}

const SET_SCAN_BYTES: usize = 16;

fn find_in_set(haystack: &[u8], set: &[bool; BYTE_VALUES]) -> Option<usize> {
    let member = |byte: &u8| set[usize::from(*byte)];
    let (chunks, remainder) = haystack.as_chunks::<SET_SCAN_BYTES>();
    for (index, chunk) in chunks.iter().enumerate() {
        if chunk.iter().fold(false, |any, byte| any | member(byte)) {
            let position = chunk
                .iter()
                .position(member)
                .expect("a chunk holding a member has its position");
            return Some(index * SET_SCAN_BYTES + position);
        }
    }
    remainder
        .iter()
        .position(member)
        .map(|position| chunks.len() * SET_SCAN_BYTES + position)
}

fn lead_matches(lead: &[u128], rest: &[u8]) -> bool {
    if rest.len() < lead.len() {
        return false;
    }
    lead.iter()
        .zip(rest)
        .all(|(mask, &byte)| u32::from(byte) <= ASCII_LAST && mask & (1 << byte) != 0)
}

struct Machine<'a> {
    program: &'a Program,
    text: &'a str,
    loops: Vec<LoopState>,
    groups: Vec<GroupState>,
    stack: Vec<Backtrack>,
    saved_groups: Vec<GroupState>,
}

fn single_matches(instruction: &Instruction, c: u32) -> bool {
    match instruction {
        Instruction::Char(expected) => c == *expected,
        Instruction::MatchAny => true,
        Instruction::MatchAnyExceptLineTerminator => !unicode::is_line_terminator(c),
        _ => unreachable!("a single character instruction"),
    }
}

fn bracket_contains(invert: bool, ascii: u128, set: &CodePointSet, c: u32) -> bool {
    let contained = if c <= ASCII_LAST {
        ascii & (1 << c) != 0
    } else {
        set.contains(c)
    };
    contained != invert
}

impl Machine<'_> {
    fn peek_left(&self, position: usize) -> Option<u32> {
        self.read(position, false).map(|(c, _)| c)
    }

    fn peek_right(&self, position: usize) -> Option<u32> {
        self.read(position, true).map(|(c, _)| c)
    }

    fn right_boundary(&self, position: usize) -> Option<usize> {
        self.read(position, true).map(|(_, next)| next)
    }

    fn left_boundary(&self, position: usize) -> Option<usize> {
        self.read(position, false).map(|(_, next)| next)
    }

    #[inline(always)]
    fn read(&self, position: usize, forward: bool) -> Option<(u32, usize)> {
        let bytes = self.text.as_bytes();
        if forward {
            let &byte = bytes.get(position)?;
            if byte.is_ascii() {
                return Some((u32::from(byte), position + 1));
            }
        } else {
            let &byte = bytes.get(position.checked_sub(1)?)?;
            if byte.is_ascii() {
                return Some((u32::from(byte), position - 1));
            }
        }
        self.read_wide(position, forward)
    }

    #[inline(never)]
    fn read_wide(&self, position: usize, forward: bool) -> Option<(u32, usize)> {
        if forward {
            let c = self.text[position..].chars().next()?;
            Some((u32::from(c), position + c.len_utf8()))
        } else {
            let c = self.text[..position].chars().next_back()?;
            Some((u32::from(c), position - c.len_utf8()))
        }
    }

    fn match_one(
        &self,
        instruction: &Instruction,
        position: usize,
        forward: bool,
    ) -> Option<usize> {
        let (c, next) = self.read(position, forward)?;
        let matched = match instruction {
            Instruction::CharSet(set) => set.contains(&c),
            Instruction::Bracket { invert, ascii, set } => {
                bracket_contains(*invert, *ascii, set, c)
            }
            _ => single_matches(instruction, c),
        };
        matched.then_some(next)
    }

    fn attempt(&mut self, mut ip: usize, mut position: usize, forward: bool) -> Option<usize> {
        let base = self.stack.len();
        loop {
            match self.step(&mut ip, &mut position, forward) {
                Flow::Continue => {}
                Flow::Goal => {
                    self.stack.truncate(base);
                    return Some(position);
                }
                Flow::Backtrack => {
                    if !self.backtrack(base, &mut ip, &mut position, forward) {
                        assert_eq!(self.stack.len(), base);
                        return None;
                    }
                }
            }
        }
    }

    fn step(&mut self, ip: &mut usize, position: &mut usize, forward: bool) -> Flow {
        let program = self.program;
        let instruction = &program.instructions[*ip];
        let next_ip = match instruction {
            Instruction::Char(_)
            | Instruction::CharSet(_)
            | Instruction::Bracket { .. }
            | Instruction::MatchAny
            | Instruction::MatchAnyExceptLineTerminator => {
                match self.match_one(instruction, *position, forward) {
                    Some(next) => {
                        *position = next;
                        Some(*ip + 1)
                    }
                    None => None,
                }
            }
            Instruction::Literal(bytes) => self
                .match_literal(bytes, position, forward)
                .then_some(*ip + 1),
            Instruction::Jump(target) => Some(*target),
            Instruction::Alt { secondary, guard } => {
                if self.guard_rejects(*guard, *position, forward) {
                    Some(*secondary)
                } else {
                    self.stack.push(Backtrack::SetPosition {
                        ip: *secondary,
                        position: *position,
                    });
                    Some(*ip + 1)
                }
            }
            Instruction::Goal => return Flow::Goal,
            Instruction::Fail => None,
            Instruction::EnterLoop(fields) => {
                self.loops[fields.id].iterations = 0;
                self.run_loop(fields, *position, *ip, forward)
            }
            Instruction::LoopAgain { begin } => {
                let Instruction::EnterLoop(fields) = &program.instructions[*begin] else {
                    unreachable!("LoopAgain refers to EnterLoop");
                };
                self.run_loop(fields, *position, *begin, forward)
            }
            Instruction::LoopOneCharacter { min, max, greedy } => {
                self.run_one_character_loop(*ip, position, *min, *max, *greedy, forward)
            }
            Instruction::Lookaround {
                negate,
                backwards,
                groups,
                continuation,
            } => self
                .run_lookaround(*ip + 1, *position, groups.clone(), *negate, !*backwards)
                .then_some(*continuation),
            _ => self
                .step_state(instruction, position, forward)
                .then_some(*ip + 1),
        };
        match next_ip {
            Some(next) => {
                *ip = next;
                Flow::Continue
            }
            None => Flow::Backtrack,
        }
    }

    fn match_literal(&self, literal: &[u8], position: &mut usize, forward: bool) -> bool {
        let (forward_bytes, backward_bytes) = literal.split_at(literal.len() / 2);
        let bytes = self.text.as_bytes();
        if forward {
            if !bytes[*position..].starts_with(forward_bytes) {
                return false;
            }
            *position += forward_bytes.len();
        } else {
            if !bytes[..*position].ends_with(backward_bytes) {
                return false;
            }
            *position -= backward_bytes.len();
        }
        true
    }

    fn step_state(
        &mut self,
        instruction: &Instruction,
        position: &mut usize,
        forward: bool,
    ) -> bool {
        match instruction {
            Instruction::StartOfLine { multiline } => match self.peek_left(*position) {
                None => true,
                Some(c) => *multiline && unicode::is_line_terminator(c),
            },
            Instruction::EndOfLine { multiline } => match self.peek_right(*position) {
                None => true,
                Some(c) => *multiline && unicode::is_line_terminator(c),
            },
            Instruction::WordBoundary {
                invert,
                ignore_case,
            } => {
                let word = if *ignore_case {
                    unicode::is_word_character_ignoring_case
                } else {
                    unicode::is_word_character
                };
                let before = self.peek_left(*position).is_some_and(word);
                let after = self.peek_right(*position).is_some_and(word);
                (before != after) != *invert
            }
            Instruction::BeginGroup(id) => {
                let state = self.groups[*id];
                self.stack.push(Backtrack::SetGroup { id: *id, state });
                let group = &mut self.groups[*id];
                if forward {
                    group.start = Some(*position);
                } else {
                    group.end = Some(*position);
                }
                true
            }
            Instruction::EndGroup(id) => {
                let group = &mut self.groups[*id];
                if forward {
                    group.end = Some(*position);
                } else {
                    group.start = Some(*position);
                }
                true
            }
            Instruction::ResetGroup(id) => {
                let state = self.groups[*id];
                self.stack.push(Backtrack::SetGroup { id: *id, state });
                self.groups[*id] = GroupState::default();
                true
            }
            Instruction::BackReference { group, ignore_case } => {
                match self.groups[*group].range() {
                    Some(range) if *ignore_case => {
                        self.back_reference_ignoring_case(range, position, forward)
                    }
                    Some(range) => self.back_reference(range, position, forward),
                    None => true,
                }
            }
            _ => unreachable!("a state instruction"),
        }
    }

    fn back_reference(&self, range: Range<usize>, position: &mut usize, forward: bool) -> bool {
        let bytes = self.text.as_bytes();
        let length = range.end - range.start;
        let compared = if forward {
            if bytes.len() - *position < length {
                return false;
            }
            *position..*position + length
        } else {
            if *position < length {
                return false;
            }
            *position - length..*position
        };
        if bytes[compared.clone()] != bytes[range] {
            return false;
        }
        *position = if forward {
            compared.end
        } else {
            compared.start
        };
        true
    }

    fn back_reference_ignoring_case(
        &self,
        range: Range<usize>,
        position: &mut usize,
        forward: bool,
    ) -> bool {
        let reference = &self.text[range];
        let mut current = *position;
        let matched = if forward {
            reference
                .chars()
                .all(|expected| self.read_folded(expected, &mut current, true))
        } else {
            reference
                .chars()
                .rev()
                .all(|expected| self.read_folded(expected, &mut current, false))
        };
        if matched {
            *position = current;
        }
        matched
    }

    fn read_folded(&self, expected: char, position: &mut usize, forward: bool) -> bool {
        let Some((c, next)) = self.read(*position, forward) else {
            return false;
        };
        let expected = u32::from(expected);
        if c != expected && unicode::fold(c) != unicode::fold(expected) {
            return false;
        }
        *position = next;
        true
    }

    fn run_lookaround(
        &mut self,
        ip: usize,
        position: usize,
        groups: Range<usize>,
        negate: bool,
        forward: bool,
    ) -> bool {
        let saved = self.saved_groups.len();
        self.saved_groups
            .extend_from_slice(&self.groups[groups.clone()]);
        let matched = self.attempt(ip, position, forward).is_some();
        if matched && !negate {
            for (offset, &state) in self.saved_groups[saved..].iter().enumerate() {
                self.stack.push(Backtrack::SetGroup {
                    id: groups.start + offset,
                    state,
                });
            }
        } else {
            self.groups[groups].copy_from_slice(&self.saved_groups[saved..]);
        }
        self.saved_groups.truncate(saved);
        matched != negate
    }

    fn enter_iteration(
        stack: &mut Vec<Backtrack>,
        position: usize,
        id: usize,
        state: &mut LoopState,
    ) {
        stack.push(Backtrack::SetLoop { id, state: *state });
        state.iterations += 1;
        state.entry = position;
    }

    fn guard_rejects(&self, guard: Option<usize>, position: usize, forward: bool) -> bool {
        let Some(index) = guard else {
            return false;
        };
        forward
            && !self
                .text
                .as_bytes()
                .get(position)
                .is_some_and(|&byte| self.program.guards[index][usize::from(byte)])
    }

    fn run_loop(
        &mut self,
        fields: &LoopFields,
        position: usize,
        ip: usize,
        forward: bool,
    ) -> Option<usize> {
        let rejected = self.guard_rejects(fields.guard, position, forward);
        let state = &mut self.loops[fields.id];
        let iteration = state.iterations;
        let take = iteration < fields.max;
        let skip = iteration >= fields.min;
        if state.entry == position && iteration > fields.min {
            return None;
        }
        match (take, skip) {
            (false, false) => None,
            (false, true) => Some(fields.exit),
            (true, false) if rejected => None,
            (true, false) => {
                Self::enter_iteration(&mut self.stack, position, fields.id, state);
                Some(ip + 1)
            }
            (true, true) if !fields.greedy => {
                let entry_before = state.entry;
                state.entry = position;
                self.stack.push(Backtrack::EnterLazyLoop {
                    ip,
                    entry_before,
                    state: *state,
                });
                Some(fields.exit)
            }
            (true, true) if rejected => Some(fields.exit),
            (true, true) => {
                self.stack.push(Backtrack::SetPosition {
                    ip: fields.exit,
                    position,
                });
                Self::enter_iteration(&mut self.stack, position, fields.id, state);
                Some(ip + 1)
            }
        }
    }

    fn repeat_one(
        &self,
        matcher: &Instruction,
        position: usize,
        count: usize,
        forward: bool,
    ) -> (usize, usize) {
        match matcher {
            Instruction::CharSet(set) => {
                self.repeat_while(position, count, forward, |c| set.contains(&c))
            }
            Instruction::Bracket { invert, ascii, set } => {
                self.repeat_while(position, count, forward, |c| {
                    bracket_contains(*invert, *ascii, set, c)
                })
            }
            Instruction::MatchAny => self.repeat_while(position, count, forward, |_| true),
            _ => self.repeat_while(position, count, forward, |c| single_matches(matcher, c)),
        }
    }

    fn repeat_while(
        &self,
        mut position: usize,
        count: usize,
        forward: bool,
        matches: impl Fn(u32) -> bool,
    ) -> (usize, usize) {
        let mut matched = 0;
        while matched < count {
            let Some((c, next)) = self.read(position, forward) else {
                break;
            };
            if !matches(c) {
                break;
            }
            position = next;
            matched += 1;
        }
        (position, matched)
    }

    fn run_one_character_loop(
        &mut self,
        ip: usize,
        position: &mut usize,
        min: usize,
        max: usize,
        greedy: bool,
        forward: bool,
    ) -> Option<usize> {
        assert!(min <= max);
        let matcher = &self.program.instructions[ip + 1];
        let (min_position, matched) = if min == 0 {
            (*position, 0)
        } else {
            self.repeat_one(matcher, *position, min, forward)
        };
        if matched < min {
            return None;
        }
        let max_position = if max == min {
            min_position
        } else {
            self.repeat_one(matcher, min_position, max - min, forward).0
        };
        let continuation = ip + 2;
        if min_position != max_position {
            self.stack.push(if greedy {
                Backtrack::GreedyOneCharacter {
                    continuation,
                    min: min_position,
                    max: max_position,
                }
            } else {
                Backtrack::LazyOneCharacter {
                    continuation,
                    min: min_position,
                    max: max_position,
                }
            });
        }
        *position = if greedy { max_position } else { min_position };
        Some(continuation)
    }

    fn step_back(&self, position: usize, forward: bool) -> Option<usize> {
        if forward {
            self.left_boundary(position)
        } else {
            self.right_boundary(position)
        }
    }

    fn continuation_fails(&self, continuation: usize, position: usize, forward: bool) -> bool {
        let instruction = &self.program.instructions[continuation];
        matches!(
            instruction,
            Instruction::Char(_)
                | Instruction::CharSet(_)
                | Instruction::Bracket { .. }
                | Instruction::MatchAny
                | Instruction::MatchAnyExceptLineTerminator
        ) && self.match_one(instruction, position, forward).is_none()
    }

    fn backtrack(
        &mut self,
        base: usize,
        ip: &mut usize,
        position: &mut usize,
        forward: bool,
    ) -> bool {
        loop {
            if self.stack.len() == base {
                return false;
            }
            let top = *self.stack.last().expect("the stack is above its base");
            match top {
                Backtrack::SetPosition {
                    ip: saved_ip,
                    position: saved_position,
                } => {
                    self.stack.pop();
                    *ip = saved_ip;
                    *position = saved_position;
                    return true;
                }
                Backtrack::SetLoop { id, state } => {
                    self.stack.pop();
                    self.loops[id] = state;
                }
                Backtrack::SetGroup { id, state } => {
                    self.stack.pop();
                    self.groups[id] = state;
                }
                Backtrack::EnterLazyLoop {
                    ip: loop_ip,
                    entry_before,
                    state,
                } => {
                    let Instruction::EnterLoop(fields) = &self.program.instructions[loop_ip] else {
                        unreachable!("a lazy loop refers to EnterLoop");
                    };
                    *ip = loop_ip + 1;
                    *position = state.entry;
                    let last = self.stack.len() - 1;
                    self.stack[last] = Backtrack::SetLoop {
                        id: fields.id,
                        state: LoopState {
                            entry: entry_before,
                            ..state
                        },
                    };
                    self.loops[fields.id] = state;
                    Self::enter_iteration(
                        &mut self.stack,
                        *position,
                        fields.id,
                        &mut self.loops[fields.id],
                    );
                    return true;
                }
                Backtrack::GreedyOneCharacter {
                    continuation,
                    min,
                    max,
                } => {
                    if max == min {
                        self.stack.pop();
                        continue;
                    }
                    let mut shorter = self
                        .step_back(max, forward)
                        .expect("a longer one-character run can give back a character");
                    while shorter != min && self.continuation_fails(continuation, shorter, forward) {
                        shorter = self
                            .step_back(shorter, forward)
                            .expect("a longer one-character run can give back a character");
                    }
                    let last = self.stack.len() - 1;
                    self.stack[last] = Backtrack::GreedyOneCharacter {
                        continuation,
                        min,
                        max: shorter,
                    };
                    *position = shorter;
                    *ip = continuation;
                    return true;
                }
                Backtrack::LazyOneCharacter {
                    continuation,
                    min,
                    max,
                } => {
                    if max == min {
                        self.stack.pop();
                        continue;
                    }
                    let mut longer = self
                        .step_back(min, !forward)
                        .expect("a shorter one-character run can take another character");
                    while longer != max && self.continuation_fails(continuation, longer, forward) {
                        longer = self
                            .step_back(longer, !forward)
                            .expect("a shorter one-character run can take another character");
                    }
                    let last = self.stack.len() - 1;
                    self.stack[last] = Backtrack::LazyOneCharacter {
                        continuation,
                        min: longer,
                        max,
                    };
                    *position = longer;
                    *ip = continuation;
                    return true;
                }
            }
        }
    }
}
