use std::ops::Range;

use super::program::{Instruction, LoopFields, Program, Start};
use super::set::ASCII_LAST;
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
    Exhausted,
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
    let mut machine = Machine {
        program,
        text,
        loops: vec![
            LoopState {
                iterations: 0,
                entry: 0,
            };
            program.loop_count
        ],
        groups: vec![GroupState::default(); program.group_count],
        stack: vec![Backtrack::Exhausted],
    };
    let mut start = 0;
    loop {
        let candidate = match &program.start {
            Start::Sequence(sequence) => find_sequence(&text.as_bytes()[start..], sequence),
            Start::AnyByte(bytes) => find_any(&text.as_bytes()[start..], bytes),
            Start::Bytes(bytes) => text.as_bytes()[start..]
                .iter()
                .position(|&byte| bytes[usize::from(byte)]),
            Start::Anywhere | Start::Anchored => Some(0),
        };
        let Some(offset) = candidate else {
            return false;
        };
        start += offset;
        if machine.attempt(0, start, true).is_some() {
            return true;
        }
        if matches!(program.start, Start::Anchored) {
            return false;
        }
        let Some(next) = machine.right_boundary(start) else {
            return false;
        };
        start = next;
    }
}

const WORD_BYTES: usize = 8;
const LOW_BITS: u64 = 0x0101_0101_0101_0101;
const HIGH_BITS: u64 = 0x8080_8080_8080_8080;

const fn word_has_byte(word: u64, byte: u8) -> bool {
    let difference = word ^ (LOW_BITS * byte as u64);
    difference.wrapping_sub(LOW_BITS) & !difference & HIGH_BITS != 0
}

fn find_any(haystack: &[u8], needles: &[u8]) -> Option<usize> {
    let mut offset = 0;
    while let Some(chunk) = haystack.get(offset..offset + WORD_BYTES) {
        let word = u64::from_le_bytes(chunk.try_into().expect("a whole word"));
        if needles.iter().any(|&needle| word_has_byte(word, needle)) {
            break;
        }
        offset += WORD_BYTES;
    }
    haystack[offset..]
        .iter()
        .position(|byte| needles.contains(byte))
        .map(|position| offset + position)
}

fn find_sequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let (&first, rest) = needle.split_first().expect("a start sequence is not empty");
    let mut offset = 0;
    loop {
        let found = offset + find_any(&haystack[offset..], &[first])?;
        if haystack[found + 1..].starts_with(rest) {
            return Some(found);
        }
        offset = found + 1;
    }
}

struct Machine<'a> {
    program: &'a Program,
    text: &'a str,
    loops: Vec<LoopState>,
    groups: Vec<GroupState>,
    stack: Vec<Backtrack>,
}

impl Machine<'_> {
    fn peek_left(&self, position: usize) -> Option<u32> {
        self.text[..position].chars().next_back().map(u32::from)
    }

    fn peek_right(&self, position: usize) -> Option<u32> {
        self.text[position..].chars().next().map(u32::from)
    }

    fn right_boundary(&self, position: usize) -> Option<usize> {
        let c = self.text[position..].chars().next()?;
        Some(position + c.len_utf8())
    }

    fn left_boundary(&self, position: usize) -> Option<usize> {
        let c = self.text[..position].chars().next_back()?;
        Some(position - c.len_utf8())
    }

    fn read(&self, position: usize, forward: bool) -> Option<(u32, usize)> {
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
            Instruction::Char(expected) => c == *expected,
            Instruction::CharSet(set) => set.contains(&c),
            Instruction::Bracket { invert, ascii, set } => {
                let contained = if c <= ASCII_LAST {
                    ascii & (1 << c) != 0
                } else {
                    set.contains(c)
                };
                contained != *invert
            }
            Instruction::MatchAny => true,
            Instruction::MatchAnyExceptLineTerminator => !unicode::is_line_terminator(c),
            _ => unreachable!("a single character instruction"),
        };
        matched.then_some(next)
    }

    fn attempt(&mut self, mut ip: usize, mut position: usize, forward: bool) -> Option<usize> {
        assert_eq!(self.stack.len(), 1);
        loop {
            match self.step(&mut ip, &mut position, forward) {
                Flow::Continue => {}
                Flow::Goal => {
                    self.stack.truncate(1);
                    return Some(position);
                }
                Flow::Backtrack => {
                    if !self.backtrack(&mut ip, &mut position, forward) {
                        assert_eq!(self.stack.len(), 1);
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
            Instruction::Jump(target) => Some(*target),
            Instruction::Alt(secondary) => {
                self.stack.push(Backtrack::SetPosition {
                    ip: *secondary,
                    position: *position,
                });
                Some(*ip + 1)
            }
            Instruction::Goal => return Flow::Goal,
            Instruction::Fail => None,
            Instruction::EnterLoop(fields) => {
                self.loops[fields.id].iterations = 0;
                self.run_loop(fields, *position, *ip)
            }
            Instruction::LoopAgain { begin } => {
                let Instruction::EnterLoop(fields) = &program.instructions[*begin] else {
                    unreachable!("LoopAgain refers to EnterLoop");
                };
                self.run_loop(fields, *position, *begin)
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
        let saved: Vec<GroupState> = self.groups[groups.clone()].to_vec();
        let outer = std::mem::replace(&mut self.stack, vec![Backtrack::Exhausted]);
        let matched = self.attempt(ip, position, forward).is_some();
        self.stack = outer;
        if matched && !negate {
            for (offset, state) in saved.into_iter().enumerate() {
                self.stack.push(Backtrack::SetGroup {
                    id: groups.start + offset,
                    state,
                });
            }
        } else {
            self.groups.splice(groups, saved);
        }
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

    fn run_loop(&mut self, fields: &LoopFields, position: usize, ip: usize) -> Option<usize> {
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
        mut position: usize,
        count: usize,
        forward: bool,
    ) -> (usize, usize) {
        let mut matched = 0;
        while matched < count {
            let Some(next) = self.match_one(matcher, position, forward) else {
                break;
            };
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
        let (min_position, matched) = self.repeat_one(matcher, *position, min, forward);
        if matched < min {
            return None;
        }
        let (max_position, _) = self.repeat_one(matcher, min_position, max - min, forward);
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

    fn backtrack(&mut self, ip: &mut usize, position: &mut usize, forward: bool) -> bool {
        loop {
            let top = *self
                .stack
                .last()
                .expect("the backtrack stack keeps its Exhausted base");
            match top {
                Backtrack::Exhausted => return false,
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
                    let shorter = if forward {
                        self.left_boundary(max)
                    } else {
                        self.right_boundary(max)
                    }
                    .expect("a longer one-character run can give back a character");
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
                    let longer = if forward {
                        self.right_boundary(min)
                    } else {
                        self.left_boundary(min)
                    }
                    .expect("a shorter one-character run can take another character");
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
