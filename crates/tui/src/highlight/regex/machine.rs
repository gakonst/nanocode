use super::{
    Anchor, Region, SearchError,
    compile::{Instruction, Program, contains_range},
    unicode,
};

pub(super) const RETRY_LIMIT_IN_MATCH: u64 = 10_000_000;
const LATIN1_LIMIT: u32 = 0x100;
pub(super) const LATIN1_NUMBER_WORDS: [u32; 6] = [0xB2, 0xB3, 0xB9, 0xBC, 0xBD, 0xBE];
const UNSET: usize = usize::MAX;

#[derive(Clone, Copy)]
enum Frame {
    Alternative {
        pc: u32,
        position: usize,
    },
    Capture {
        group: u32,
        start: usize,
        end: usize,
    },
    EmptyCheck {
        id: u32,
        position: usize,
    },
    Mark {
        id: u32,
        position: usize,
    },
    RepeatCount {
        id: u32,
        count: u32,
    },
    Void,
}

#[derive(Default)]
pub(crate) struct Searcher {
    stack: Vec<Frame>,
    starts: Vec<usize>,
    ends: Vec<usize>,
}

pub(super) fn is_word(character: char) -> bool {
    if character.is_ascii() {
        return character.is_ascii_alphanumeric() || character == '_';
    }
    let code = character as u32;
    if code < LATIN1_LIMIT && LATIN1_NUMBER_WORDS.contains(&code) {
        return true;
    }
    contains_range(unicode::WORD, code)
}

#[inline]
fn char_at(text: &str, position: usize) -> Option<char> {
    text[position..].chars().next()
}

#[inline]
fn char_before(text: &str, position: usize) -> Option<char> {
    text[..position].chars().next_back()
}

fn word_at(text: &str, position: usize) -> bool {
    char_at(text, position).is_some_and(is_word)
}

fn word_before(text: &str, position: usize) -> bool {
    char_before(text, position).is_some_and(is_word)
}

fn assertion(anchor: Anchor, text: &str, position: usize, search_start: usize) -> bool {
    let bytes = text.as_bytes();
    let end = text.len();
    match anchor {
        Anchor::BeginLine => position == 0 || (position != end && bytes[position - 1] == b'\n'),
        Anchor::EndLine => position == end || bytes[position] == b'\n',
        Anchor::BeginBuffer => position == 0,
        Anchor::EndBuffer => position == end,
        Anchor::SemiEndBuffer => {
            position == end || (bytes[position] == b'\n' && position + 1 == end)
        }
        Anchor::BeginPosition => position == search_start,
        Anchor::WordBoundary => {
            if position == 0 {
                position < end && word_at(text, position)
            } else if position == end {
                word_before(text, position)
            } else {
                word_at(text, position) != word_before(text, position)
            }
        }
        Anchor::NotWordBoundary => {
            if position == 0 {
                !(position < end && word_at(text, position))
            } else if position == end {
                !word_before(text, position)
            } else {
                word_at(text, position) == word_before(text, position)
            }
        }
    }
}

fn step_back(text: &str, position: usize, characters: u32) -> Option<usize> {
    let mut position = position;
    for _ in 0..characters {
        let character = char_before(text, position)?;
        position -= character.len_utf8();
    }
    Some(position)
}

impl Searcher {
    pub(super) fn search(
        &mut self,
        program: &Program,
        text: &str,
        start: usize,
        region: &mut Region,
    ) -> Result<bool, SearchError> {
        let registers = program.groups + 1;
        self.starts.resize(registers, UNSET);
        self.ends.resize(registers, UNSET);
        let bytes = text.as_bytes();
        for required in &program.required {
            if !bytes[start..].iter().any(|&byte| required.contains(byte)) {
                return Ok(false);
            }
        }
        let mut position = start;
        loop {
            if let Some(first) = &program.first_bytes {
                match bytes[position..]
                    .iter()
                    .position(|&byte| first.contains(byte))
                {
                    Some(offset) => position += offset,
                    None => return Ok(false),
                }
            }
            if let Some(end) = self.match_at(program, text, start, position)? {
                region.positions.clear();
                region.positions.push(Some((position, end)));
                for group in 1..registers {
                    let (group_start, group_end) = (self.starts[group], self.ends[group]);
                    region.positions.push(
                        (group_start != UNSET && group_end != UNSET)
                            .then_some((group_start, group_end)),
                    );
                }
                return Ok(true);
            }
            let Some(character) = char_at(text, position) else {
                return Ok(false);
            };
            position += character.len_utf8();
        }
    }

    fn fail(&mut self, retries: &mut u64) -> Result<Option<(u32, usize)>, SearchError> {
        while let Some(frame) = self.stack.pop() {
            match frame {
                Frame::Alternative { pc, position } => {
                    *retries += 1;
                    if *retries >= RETRY_LIMIT_IN_MATCH {
                        return Err(SearchError::RetryLimitInMatch);
                    }
                    return Ok(Some((pc, position)));
                }
                Frame::Capture { group, start, end } => {
                    self.starts[group as usize] = start;
                    self.ends[group as usize] = end;
                }
                Frame::EmptyCheck { .. }
                | Frame::Mark { .. }
                | Frame::RepeatCount { .. }
                | Frame::Void => {}
            }
        }
        Ok(None)
    }

    fn repeat_count(&self, id: u32) -> u32 {
        for frame in self.stack.iter().rev() {
            if let Frame::RepeatCount {
                id: frame_id,
                count,
            } = *frame
                && frame_id == id
            {
                return count;
            }
        }
        unreachable!("a repeat increment follows its repeat start")
    }

    fn empty_check_position(&self, id: u32) -> usize {
        for frame in self.stack.iter().rev() {
            if let Frame::EmptyCheck {
                id: frame_id,
                position,
            } = *frame
                && frame_id == id
            {
                return position;
            }
        }
        unreachable!("an empty check end follows its start")
    }

    fn cut_to_mark(&mut self, id: u32) -> usize {
        for frame in self.stack.iter_mut().rev() {
            match *frame {
                Frame::Mark {
                    id: frame_id,
                    position,
                } if frame_id == id => {
                    *frame = Frame::Void;
                    return position;
                }
                Frame::Alternative { .. } | Frame::EmptyCheck { .. } => *frame = Frame::Void,
                Frame::Mark { .. }
                | Frame::Capture { .. }
                | Frame::RepeatCount { .. }
                | Frame::Void => {}
            }
        }
        unreachable!("a cut finds its mark")
    }

    fn pop_to_mark(&mut self, id: u32) {
        while let Some(frame) = self.stack.pop() {
            match frame {
                Frame::Mark { id: frame_id, .. } if frame_id == id => return,
                Frame::Capture { group, start, end } => {
                    self.starts[group as usize] = start;
                    self.ends[group as usize] = end;
                }
                _ => {}
            }
        }
        unreachable!("a pop to mark finds its mark")
    }

    fn match_at(
        &mut self,
        program: &Program,
        text: &str,
        search_start: usize,
        start: usize,
    ) -> Result<Option<usize>, SearchError> {
        self.stack.clear();
        self.starts.fill(UNSET);
        self.ends.fill(UNSET);
        let bytes = text.as_bytes();
        let mut pc: u32 = 0;
        let mut position = start;
        let mut retries: u64 = 0;
        loop {
            let matched = match program.instructions[pc as usize] {
                Instruction::Literal { start, length } => {
                    let literal = &program.literals[start as usize..(start + length) as usize];
                    if bytes[position..].starts_with(literal) {
                        position += literal.len();
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Instruction::Set(index) => match char_at(text, position) {
                    Some(character) if program.sets[index as usize].contains(character as u32) => {
                        position += character.len_utf8();
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Instruction::Word => match char_at(text, position) {
                    Some(character) if is_word(character) => {
                        position += character.len_utf8();
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Instruction::NotWord => match char_at(text, position) {
                    Some(character) if !is_word(character) => {
                        position += character.len_utf8();
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Instruction::Any => match char_at(text, position) {
                    Some(character) if character != '\n' => {
                        position += character.len_utf8();
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Instruction::AnyMultiline => match char_at(text, position) {
                    Some(character) => {
                        position += character.len_utf8();
                        pc += 1;
                        true
                    }
                    None => false,
                },
                Instruction::AnyStar | Instruction::AnyStarMultiline => {
                    let multiline = matches!(
                        program.instructions[pc as usize],
                        Instruction::AnyStarMultiline
                    );
                    pc += 1;
                    let mut reached_end = true;
                    while let Some(character) = char_at(text, position) {
                        self.stack.push(Frame::Alternative { pc, position });
                        if !multiline && character == '\n' {
                            reached_end = false;
                            break;
                        }
                        position += character.len_utf8();
                    }
                    reached_end
                }
                Instruction::AnyStarPeekNext { byte, multiline } => {
                    pc += 1;
                    while let Some(character) = char_at(text, position) {
                        if bytes[position] == byte {
                            self.stack.push(Frame::Alternative { pc, position });
                        }
                        if !multiline && character == '\n' {
                            break;
                        }
                        position += character.len_utf8();
                    }
                    position >= bytes.len()
                }
                Instruction::PushOrJumpExact { byte, target } => {
                    if bytes.get(position) == Some(&byte) {
                        self.stack.push(Frame::Alternative {
                            pc: target,
                            position,
                        });
                        pc += 1;
                    } else {
                        pc = target;
                    }
                    true
                }
                Instruction::PushIfPeekNext { byte, target } => {
                    if bytes.get(position) == Some(&byte) {
                        self.stack.push(Frame::Alternative {
                            pc: target,
                            position,
                        });
                    }
                    pc += 1;
                    true
                }
                Instruction::Push(target) => {
                    self.stack.push(Frame::Alternative {
                        pc: target,
                        position,
                    });
                    pc += 1;
                    true
                }
                Instruction::Jump(target) => {
                    pc = target;
                    true
                }
                Instruction::CaptureStart(group) => {
                    let index = group as usize;
                    self.stack.push(Frame::Capture {
                        group,
                        start: self.starts[index],
                        end: self.ends[index],
                    });
                    self.starts[index] = position;
                    pc += 1;
                    true
                }
                Instruction::CaptureEnd(group) => {
                    let index = group as usize;
                    self.stack.push(Frame::Capture {
                        group,
                        start: self.starts[index],
                        end: self.ends[index],
                    });
                    self.ends[index] = position;
                    pc += 1;
                    true
                }
                Instruction::Backref { group, ignore_case } => {
                    assert!(
                        !ignore_case,
                        "ignore-case backreferences are rejected when compiling"
                    );
                    let index = group as usize;
                    let (group_start, group_end) = (self.starts[index], self.ends[index]);
                    if group_start == UNSET || group_end == UNSET {
                        false
                    } else if group_end <= group_start {
                        pc += 1;
                        true
                    } else if bytes[position..].starts_with(&bytes[group_start..group_end]) {
                        position += group_end - group_start;
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Instruction::EmptyCheckStart(id) => {
                    self.stack.push(Frame::EmptyCheck { id, position });
                    pc += 1;
                    true
                }
                Instruction::EmptyCheckEnd(id) => {
                    pc += if self.empty_check_position(id) == position {
                        2
                    } else {
                        1
                    };
                    true
                }
                Instruction::Mark { id, save_position } => {
                    let _ = save_position;
                    self.stack.push(Frame::Mark { id, position });
                    pc += 1;
                    true
                }
                Instruction::CutToMark {
                    id,
                    restore_position,
                } => {
                    let saved = self.cut_to_mark(id);
                    if restore_position {
                        position = saved;
                    }
                    pc += 1;
                    true
                }
                Instruction::PopToMark(id) => {
                    self.pop_to_mark(id);
                    pc += 1;
                    true
                }
                Instruction::Pop => {
                    self.stack.pop().expect("a pop has a frame to remove");
                    pc += 1;
                    true
                }
                Instruction::Fail => false,
                Instruction::StepBack(characters) => match step_back(text, position, characters) {
                    Some(back) => {
                        position = back;
                        pc += 1;
                        true
                    }
                    None => false,
                },
                Instruction::Repeat { id, greedy, exit } => {
                    self.stack.push(Frame::RepeatCount { id, count: 0 });
                    if program.repeats[id as usize].lower == 0 {
                        if greedy {
                            self.stack.push(Frame::Alternative { pc: exit, position });
                            pc += 1;
                        } else {
                            self.stack.push(Frame::Alternative {
                                pc: pc + 1,
                                position,
                            });
                            pc = exit;
                        }
                    } else {
                        pc += 1;
                    }
                    true
                }
                Instruction::RepeatIncrement { id, greedy } => {
                    let range = &program.repeats[id as usize];
                    let count = self.repeat_count(id) + 1;
                    if greedy {
                        if range.upper.is_some_and(|upper| count >= upper) {
                            pc += 1;
                        } else if count >= range.lower {
                            self.stack.push(Frame::Alternative {
                                pc: pc + 1,
                                position,
                            });
                            pc = range.body;
                        } else {
                            pc = range.body;
                        }
                        self.stack.push(Frame::RepeatCount { id, count });
                    } else {
                        self.stack.push(Frame::RepeatCount { id, count });
                        if range.upper == Some(count) {
                            pc += 1;
                        } else if count >= range.lower {
                            self.stack.push(Frame::Alternative {
                                pc: range.body,
                                position,
                            });
                            pc += 1;
                        } else {
                            pc = range.body;
                        }
                    }
                    true
                }
                Instruction::Assert(anchor) => {
                    if assertion(anchor, text, position, search_start) {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Instruction::Match => return Ok(Some(position)),
            };
            if !matched {
                match self.fail(&mut retries)? {
                    Some((next_pc, next_position)) => {
                        pc = next_pc;
                        position = next_position;
                    }
                    None => return Ok(None),
                }
            }
        }
    }
}
