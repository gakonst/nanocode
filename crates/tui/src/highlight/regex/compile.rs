use std::sync::OnceLock;

use super::{
    Anchor, Class, Emptiness, Error, LookBehindLength, Node, Repeat, class::CodePointSet, unicode,
};

const QUANTIFIER_EXPAND_LIMIT_SIZE: u64 = 10;
const BITSET_WORD_BITS: u32 = u64::BITS;
const SINGLE_OPERATION: u64 = 1;
const LOOK_AHEAD_OPERATIONS: u64 = 2;
const NEGATIVE_LOOK_AHEAD_OPERATIONS: u64 = 5;
const LOOK_BEHIND_OPERATIONS: u64 = 3;
const NEGATIVE_LOOK_BEHIND_OPERATIONS: u64 = 6;
const CAPTURE_OPERATIONS: u64 = 2;
const ATOMIC_OPERATIONS: u64 = 2;
const STRICT_REPEAT_OPERATIONS: u64 = 3;
const ALTERNATIVE_OPERATIONS: u64 = 2;
const EMPTY_CHECK_OPERATIONS: u64 = 2;
const LOOP_OPERATIONS: u64 = 2;
const ASCII_LIMIT: u32 = 0x80;
const BYTE_VALUES: usize = 256;

#[derive(Clone, Copy, Debug)]
pub(super) enum Instruction {
    Literal { start: u32, length: u32 },
    Set(u32),
    Word,
    NotWord,
    Any,
    AnyMultiline,
    AnyStar,
    AnyStarMultiline,
    AnyStarPeekNext { byte: u8, multiline: bool },
    Push(u32),
    PushOrJumpExact { byte: u8, target: u32 },
    PushIfPeekNext { byte: u8, target: u32 },
    Jump(u32),
    CaptureStart(u32),
    CaptureEnd(u32),
    Backref { group: u32, ignore_case: bool },
    EmptyCheckStart(u32),
    EmptyCheckEnd(u32),
    Mark { id: u32, save_position: bool },
    CutToMark { id: u32, restore_position: bool },
    PopToMark(u32),
    Pop,
    Fail,
    StepBack(u32),
    Repeat { id: u32, greedy: bool, exit: u32 },
    RepeatIncrement { id: u32, greedy: bool },
    Assert(Anchor),
    Match,
}

pub(super) struct Set {
    ascii: [u64; 2],
    ranges: Box<[(u32, u32)]>,
}

impl Set {
    fn new(set: &CodePointSet) -> Self {
        let mut ascii = [0_u64; 2];
        let mut ranges = Vec::new();
        for &(first, last) in set.ranges() {
            for code in first..=last.min(ASCII_LIMIT - 1) {
                ascii[(code / BITSET_WORD_BITS) as usize] |= 1 << (code % BITSET_WORD_BITS);
            }
            if last >= ASCII_LIMIT {
                ranges.push((first.max(ASCII_LIMIT), last));
            }
        }
        Self {
            ascii,
            ranges: ranges.into_boxed_slice(),
        }
    }

    #[inline]
    pub(super) fn contains(&self, code: u32) -> bool {
        if code < ASCII_LIMIT {
            return self.ascii[(code / BITSET_WORD_BITS) as usize]
                & (1 << (code % BITSET_WORD_BITS))
                != 0;
        }
        contains_range(&self.ranges, code)
    }
}

pub(super) fn contains_range(ranges: &[(u32, u32)], code: u32) -> bool {
    let index = ranges.partition_point(|&(_, last)| last < code);
    index < ranges.len() && ranges[index].0 <= code
}

pub(super) struct RepeatRange {
    pub(super) lower: u32,
    pub(super) upper: Option<u32>,
    pub(super) body: u32,
}

#[derive(PartialEq, Eq)]
pub(super) struct FirstBytes {
    bits: [u64; 4],
}

impl FirstBytes {
    #[inline]
    pub(super) fn contains(&self, byte: u8) -> bool {
        self.bits[(u32::from(byte) / BITSET_WORD_BITS) as usize]
            & (1 << (u32::from(byte) % BITSET_WORD_BITS))
            != 0
    }
}

pub(super) struct Program {
    pub(super) instructions: Vec<Instruction>,
    pub(super) sets: Vec<Set>,
    pub(super) literals: Vec<u8>,
    pub(super) repeats: Vec<RepeatRange>,
    pub(super) groups: usize,
    pub(super) first_bytes: Option<FirstBytes>,
    pub(super) required: Vec<FirstBytes>,
}

struct Compiler {
    program: Program,
    marks: u32,
    empty_checks: u32,
}

pub(super) fn compile(root: &Node, groups: usize) -> Result<Program, Error> {
    let mut compiler = Compiler {
        program: Program {
            instructions: Vec::new(),
            sets: Vec::new(),
            literals: Vec::new(),
            repeats: Vec::new(),
            groups,
            first_bytes: None,
            required: Vec::new(),
        },
        marks: 0,
        empty_checks: 0,
    };
    compiler.node(root)?;
    compiler.emit(Instruction::Match);
    let (bytes, nullable) = first_bytes(root);
    if !nullable {
        compiler.program.first_bytes = Some(FirstBytes { bits: bytes });
    }
    compiler.program.required = required_sets(root);
    Ok(compiler.program)
}

impl Compiler {
    fn position(&self) -> u32 {
        u32::try_from(self.program.instructions.len())
            .expect("programs are smaller than four billion instructions")
    }

    fn emit(&mut self, instruction: Instruction) -> u32 {
        let position = self.position();
        self.program.instructions.push(instruction);
        position
    }

    fn patch(&mut self, at: u32, target: u32) {
        let slot = &mut self.program.instructions[at as usize];
        *slot = match *slot {
            Instruction::Push(_) => Instruction::Push(target),
            Instruction::PushOrJumpExact { byte, .. } => {
                Instruction::PushOrJumpExact { byte, target }
            }
            Instruction::PushIfPeekNext { byte, .. } => {
                Instruction::PushIfPeekNext { byte, target }
            }
            Instruction::Jump(_) => Instruction::Jump(target),
            Instruction::Repeat { id, greedy, .. } => Instruction::Repeat {
                id,
                greedy,
                exit: target,
            },
            other => unreachable!("only jumps are patched, not {other:?}"),
        };
    }

    const fn mark(&mut self) -> u32 {
        self.marks += 1;
        self.marks
    }

    fn set(&mut self, set: &CodePointSet) -> u32 {
        self.program.sets.push(Set::new(set));
        u32::try_from(self.program.sets.len() - 1).expect("fewer than four billion sets")
    }

    fn node(&mut self, node: &Node) -> Result<(), Error> {
        match node {
            Node::Empty => {}
            Node::Fail => {
                self.emit(Instruction::Fail);
            }
            Node::String {
                text, ignore_case, ..
            } => {
                assert!(
                    !ignore_case,
                    "ignore-case strings are unravelled before compiling"
                );
                if !text.is_empty() {
                    let start =
                        u32::try_from(self.program.literals.len()).expect("literals fit u32");
                    self.program.literals.extend_from_slice(text.as_bytes());
                    let length = u32::try_from(text.len()).expect("literals fit u32");
                    self.emit(Instruction::Literal { start, length });
                }
            }
            Node::Class(class) => {
                let set = resolved(class);
                let index = self.set(&set);
                self.emit(Instruction::Set(index));
            }
            Node::Word { negated } => {
                self.emit(if *negated {
                    Instruction::NotWord
                } else {
                    Instruction::Word
                });
            }
            Node::Any { multiline } => {
                self.emit(if *multiline {
                    Instruction::AnyMultiline
                } else {
                    Instruction::Any
                });
            }
            Node::Anchor(anchor) => {
                self.emit(Instruction::Assert(*anchor));
            }
            Node::LookAhead {
                negated: false,
                body,
            } => {
                let id = self.mark();
                self.emit(Instruction::Mark {
                    id,
                    save_position: true,
                });
                self.node(body)?;
                self.emit(Instruction::CutToMark {
                    id,
                    restore_position: true,
                });
            }
            Node::LookAhead {
                negated: true,
                body,
            } => {
                let push = self.emit(Instruction::Push(0));
                let id = self.mark();
                self.emit(Instruction::Mark {
                    id,
                    save_position: false,
                });
                self.node(body)?;
                self.emit(Instruction::PopToMark(id));
                self.emit(Instruction::Pop);
                self.emit(Instruction::Fail);
                let end = self.position();
                self.patch(push, end);
            }
            Node::LookBehind {
                negated,
                length,
                body,
            } => {
                let LookBehindLength::Fixed(length) = *length else {
                    unreachable!("look-behind lengths are known after tuning")
                };
                let id = self.mark();
                self.emit(Instruction::Mark {
                    id,
                    save_position: false,
                });
                if *negated {
                    let push = self.emit(Instruction::Push(0));
                    if length != 0 {
                        self.emit(Instruction::StepBack(length));
                    }
                    self.node(body)?;
                    self.emit(Instruction::PopToMark(id));
                    self.emit(Instruction::Fail);
                    let pop = self.emit(Instruction::Pop);
                    self.patch(push, pop);
                } else {
                    if length != 0 {
                        self.emit(Instruction::StepBack(length));
                    }
                    self.node(body)?;
                    self.emit(Instruction::CutToMark {
                        id,
                        restore_position: false,
                    });
                }
            }
            Node::Backref { group, ignore_case } => {
                if *ignore_case {
                    return Err(Error::Unsupported("ignore-case backreference"));
                }
                self.emit(Instruction::Backref {
                    group: u32::try_from(*group).expect("group numbers fit u32"),
                    ignore_case: false,
                });
            }
            Node::Capture { group, body } => {
                let group = u32::try_from(*group).expect("group numbers fit u32");
                self.emit(Instruction::CaptureStart(group));
                self.node(body)?;
                self.emit(Instruction::CaptureEnd(group));
            }
            Node::Atomic(body) => {
                let id = self.mark();
                self.emit(Instruction::Mark {
                    id,
                    save_position: false,
                });
                self.node(body)?;
                self.emit(Instruction::CutToMark {
                    id,
                    restore_position: false,
                });
            }
            Node::Options(body) => self.node(body)?,
            Node::Concat(nodes) => {
                for node in nodes {
                    self.node(node)?;
                }
            }
            Node::Alternation(nodes) => {
                let mut jumps = Vec::with_capacity(nodes.len());
                for (index, node) in nodes.iter().enumerate() {
                    if index + 1 < nodes.len() {
                        let push = self.emit(Instruction::Push(0));
                        self.node(node)?;
                        jumps.push(self.emit(Instruction::Jump(0)));
                        let next = self.position();
                        self.patch(push, next);
                    } else {
                        self.node(node)?;
                    }
                }
                let end = self.position();
                for jump in jumps {
                    self.patch(jump, end);
                }
            }
            Node::Repeat(repeat) => self.repeat(repeat)?,
        }
        Ok(())
    }

    fn times(&mut self, body: &Node, count: u32) -> Result<(), Error> {
        for _ in 0..count {
            self.node(body)?;
        }
        Ok(())
    }

    fn checked_body(&mut self, repeat: &Repeat) -> Result<(), Error> {
        if repeat.emptiness == Emptiness::NotEmpty {
            return self.node(&repeat.body);
        }
        let id = self.empty_checks;
        self.empty_checks += 1;
        self.emit(Instruction::EmptyCheckStart(id));
        self.node(&repeat.body)?;
        self.emit(Instruction::EmptyCheckEnd(id));
        Ok(())
    }

    fn repeat(&mut self, repeat: &Repeat) -> Result<(), Error> {
        let body_length = op_length(&repeat.body);
        if body_length == 0 {
            return Ok(());
        }
        let lower = u64::from(repeat.lower);
        let infinite = repeat.upper.is_none();
        if let Node::Any { multiline } = repeat.body.as_ref()
            && repeat.greedy
            && infinite
            && (repeat.lower <= 1 || body_length * lower <= QUANTIFIER_EXPAND_LIMIT_SIZE)
        {
            self.times(&repeat.body, repeat.lower)?;
            self.emit(match repeat.next_head_exact {
                Some(byte) => Instruction::AnyStarPeekNext {
                    byte,
                    multiline: *multiline,
                },
                None if *multiline => Instruction::AnyStarMultiline,
                None => Instruction::AnyStar,
            });
            return Ok(());
        }
        if infinite && (repeat.lower <= 1 || body_length * lower <= QUANTIFIER_EXPAND_LIMIT_SIZE) {
            let entry_jump = if repeat.lower == 1 && body_length > QUANTIFIER_EXPAND_LIMIT_SIZE {
                Some(self.emit(Instruction::Jump(0)))
            } else {
                self.times(&repeat.body, repeat.lower)?;
                None
            };
            if repeat.greedy {
                let push = self.emit(match (repeat.head_exact, repeat.next_head_exact) {
                    (Some(byte), _) => Instruction::PushOrJumpExact { byte, target: 0 },
                    (None, Some(byte)) => Instruction::PushIfPeekNext { byte, target: 0 },
                    (None, None) => Instruction::Push(0),
                });
                let body = self.position();
                if let Some(jump) = entry_jump {
                    self.patch(jump, body);
                }
                self.checked_body(repeat)?;
                self.emit(Instruction::Jump(push));
                let exit = self.position();
                self.patch(push, exit);
            } else {
                let skip = self.emit(Instruction::Jump(0));
                let body = self.position();
                if let Some(jump) = entry_jump {
                    self.patch(jump, body);
                }
                self.checked_body(repeat)?;
                let push = self.emit(Instruction::Push(body));
                self.patch(skip, push);
            }
            return Ok(());
        }
        if repeat.upper == Some(0) {
            return Ok(());
        }
        if let Some(upper) = repeat.upper
            && repeat.greedy
            && (upper == 1
                || (body_length + SINGLE_OPERATION) * u64::from(upper)
                    <= QUANTIFIER_EXPAND_LIMIT_SIZE)
        {
            self.times(&repeat.body, repeat.lower)?;
            let mut pushes = Vec::new();
            for _ in repeat.lower..upper {
                pushes.push(self.emit(Instruction::Push(0)));
                self.node(&repeat.body)?;
            }
            let end = self.position();
            for push in pushes {
                self.patch(push, end);
            }
            return Ok(());
        }
        if !repeat.greedy && repeat.upper == Some(1) && repeat.lower == 0 {
            let push = self.emit(Instruction::Push(0));
            let jump = self.emit(Instruction::Jump(0));
            let body = self.position();
            self.patch(push, body);
            self.node(&repeat.body)?;
            let end = self.position();
            self.patch(jump, end);
            return Ok(());
        }
        let id =
            u32::try_from(self.program.repeats.len()).expect("fewer than four billion repeats");
        let start = self.emit(Instruction::Repeat {
            id,
            greedy: repeat.greedy,
            exit: 0,
        });
        let body = self.position();
        self.program.repeats.push(RepeatRange {
            lower: repeat.lower,
            upper: repeat.upper,
            body,
        });
        self.checked_body(repeat)?;
        self.emit(Instruction::RepeatIncrement {
            id,
            greedy: repeat.greedy,
        });
        let exit = self.position();
        self.patch(start, exit);
        Ok(())
    }
}

fn resolved(class: &Class) -> CodePointSet {
    if class.negated {
        class.set.complement()
    } else {
        class.set.clone()
    }
}

fn string_op_length(text: &str) -> u64 {
    let mut segments = 0;
    let mut previous = 0;
    for character in text.chars() {
        let length = character.len_utf8();
        if length != previous {
            segments += 1;
            previous = length;
        }
    }
    segments
}

const fn strict_real(node: &Node) -> bool {
    match node {
        Node::String { text, .. } => !text.is_empty(),
        Node::Class(_) | Node::Word { .. } | Node::Any { .. } => true,
        _ => false,
    }
}

fn op_length(node: &Node) -> u64 {
    match node {
        Node::Empty => 0,
        Node::Fail
        | Node::Class(_)
        | Node::Word { .. }
        | Node::Any { .. }
        | Node::Anchor(_)
        | Node::Backref { .. } => SINGLE_OPERATION,
        Node::String { text, .. } => string_op_length(text),
        Node::LookAhead { negated, body } => {
            op_length(body)
                + if *negated {
                    NEGATIVE_LOOK_AHEAD_OPERATIONS
                } else {
                    LOOK_AHEAD_OPERATIONS
                }
        }
        Node::LookBehind { negated, body, .. } => {
            op_length(body)
                + if *negated {
                    NEGATIVE_LOOK_BEHIND_OPERATIONS
                } else {
                    LOOK_BEHIND_OPERATIONS
                }
        }
        Node::Capture { body, .. } => op_length(body) + CAPTURE_OPERATIONS,
        Node::Atomic(body) => match body.as_ref() {
            Node::Repeat(repeat)
                if repeat.upper.is_none()
                    && repeat.lower <= 1
                    && repeat.greedy
                    && strict_real(&repeat.body) =>
            {
                let length = op_length(&repeat.body);
                u64::from(repeat.lower) * length + length + STRICT_REPEAT_OPERATIONS
            }
            other => op_length(other) + ATOMIC_OPERATIONS,
        },
        Node::Options(body) => op_length(body),
        Node::Concat(nodes) => nodes.iter().map(op_length).sum(),
        Node::Alternation(nodes) => {
            nodes.iter().map(op_length).sum::<u64>()
                + ALTERNATIVE_OPERATIONS * (nodes.len() as u64 - 1)
        }
        Node::Repeat(repeat) => repeat_op_length(repeat),
    }
}

fn repeat_op_length(repeat: &Repeat) -> u64 {
    let length = op_length(&repeat.body);
    if length == 0 {
        return 0;
    }
    let lower = u64::from(repeat.lower);
    let infinite = repeat.upper.is_none();
    if matches!(repeat.body.as_ref(), Node::Any { .. })
        && repeat.greedy
        && infinite
        && (repeat.lower <= 1 || length * lower <= QUANTIFIER_EXPAND_LIMIT_SIZE)
    {
        return SINGLE_OPERATION + length * lower;
    }
    let checked = length
        + if repeat.emptiness == Emptiness::MayBeEmpty {
            EMPTY_CHECK_OPERATIONS
        } else {
            0
        };
    if infinite && (repeat.lower <= 1 || length * lower <= QUANTIFIER_EXPAND_LIMIT_SIZE) {
        let entry = if repeat.lower == 1 && length > QUANTIFIER_EXPAND_LIMIT_SIZE {
            SINGLE_OPERATION
        } else {
            length * lower
        };
        return entry + checked + LOOP_OPERATIONS;
    }
    match repeat.upper {
        Some(0) => 0,
        Some(upper)
            if repeat.greedy
                && (upper == 1
                    || (length + SINGLE_OPERATION) * u64::from(upper)
                        <= QUANTIFIER_EXPAND_LIMIT_SIZE) =>
        {
            length * lower + (length + SINGLE_OPERATION) * u64::from(upper - repeat.lower)
        }
        Some(1) if !repeat.greedy && repeat.lower == 0 => length + LOOP_OPERATIONS,
        _ => checked + LOOP_OPERATIONS,
    }
}

type ByteBits = [u64; 4];

fn add_byte(bits: &mut ByteBits, byte: u8) {
    bits[(u32::from(byte) / BITSET_WORD_BITS) as usize] |=
        1 << (u32::from(byte) % BITSET_WORD_BITS);
}

const fn lead_byte(code: u32) -> u8 {
    let mut buffer = [0_u8; 4];
    let character = char::from_u32(code).expect("lead bytes are computed for scalar values");
    character.encode_utf8(&mut buffer);
    buffer[0]
}

const UTF8_LENGTH_BOUNDARIES: [(u32, u32); 5] = [
    (0, 0x7F),
    (0x80, 0x7FF),
    (0x800, 0xD7FF),
    (0xE000, 0xFFFF),
    (0x1_0000, 0x10_FFFF),
];

fn add_set_bytes(bits: &mut ByteBits, ranges: &[(u32, u32)]) {
    for &(first, last) in ranges {
        for (low, high) in UTF8_LENGTH_BOUNDARIES {
            let from = first.max(low);
            let to = last.min(high);
            if from > to {
                continue;
            }
            for byte in lead_byte(from)..=lead_byte(to) {
                add_byte(bits, byte);
            }
        }
    }
}

const fn all_bytes() -> ByteBits {
    [u64::MAX; 4]
}

fn first_bytes(node: &Node) -> (ByteBits, bool) {
    let mut bits = [0_u64; 4];
    match node {
        Node::Empty | Node::Anchor(_) | Node::LookAhead { .. } | Node::LookBehind { .. } => {
            (bits, true)
        }
        Node::Fail => (bits, false),
        Node::Backref { .. } => (all_bytes(), true),
        Node::String { text, .. } => match text.as_bytes().first() {
            Some(&byte) => {
                add_byte(&mut bits, byte);
                (bits, false)
            }
            None => (bits, true),
        },
        Node::Class(class) => {
            add_set_bytes(&mut bits, resolved(class).ranges());
            (bits, false)
        }
        Node::Word { negated } => (word_first_bytes(*negated), false),
        Node::Any { multiline } => {
            let mut set = CodePointSet::from_ranges(&[(0, super::class::CODE_POINT_MAX)]);
            if !multiline {
                set = set.intersection(&CodePointSet::from_ranges(&[
                    (0, 9),
                    (11, super::class::CODE_POINT_MAX),
                ]));
            }
            add_set_bytes(&mut bits, set.ranges());
            (bits, false)
        }
        Node::Capture { body, .. } | Node::Atomic(body) | Node::Options(body) => first_bytes(body),
        Node::Repeat(repeat) => {
            let (bits, nullable) = first_bytes(&repeat.body);
            (bits, nullable || repeat.lower == 0)
        }
        Node::Concat(nodes) => {
            for node in nodes {
                let (node_bits, nullable) = first_bytes(node);
                for (target, source) in bits.iter_mut().zip(node_bits) {
                    *target |= source;
                }
                if !nullable {
                    return (bits, false);
                }
            }
            (bits, true)
        }
        Node::Alternation(nodes) => {
            let mut any_nullable = false;
            for node in nodes {
                let (node_bits, nullable) = first_bytes(node);
                for (target, source) in bits.iter_mut().zip(node_bits) {
                    *target |= source;
                }
                any_nullable |= nullable;
            }
            (bits, any_nullable)
        }
    }
}

const _: () = assert!(BYTE_VALUES == 4 * 64);

const REQUIRED_SETS_MAX: usize = 4;
const REQUIRED_SET_BYTES_MAX: u32 = 64;

fn count_bits(bits: &ByteBits) -> u32 {
    bits.iter().map(|word| word.count_ones()).sum()
}

fn required_bytes(node: &Node, sets: &mut Vec<ByteBits>) {
    match node {
        Node::String { text, .. } => {
            if let Some(&byte) = text.as_bytes().first() {
                let mut bits = [0_u64; 4];
                add_byte(&mut bits, byte);
                sets.push(bits);
            }
        }
        Node::Class(_) | Node::Word { .. } => {
            let (bits, _) = first_bytes(node);
            sets.push(bits);
        }
        Node::Concat(nodes) => {
            for node in nodes {
                required_bytes(node, sets);
            }
        }
        Node::Alternation(nodes) => {
            let mut union = [0_u64; 4];
            for node in nodes {
                let mut branch = Vec::new();
                required_bytes(node, &mut branch);
                let Some(first) = branch.first() else {
                    return;
                };
                for (target, source) in union.iter_mut().zip(first) {
                    *target |= source;
                }
            }
            sets.push(union);
        }
        Node::Repeat(repeat) if repeat.lower > 0 => required_bytes(&repeat.body, sets),
        Node::Capture { body, .. } | Node::Atomic(body) | Node::Options(body) => {
            required_bytes(body, sets)
        }
        Node::LookAhead {
            negated: false,
            body,
        } => required_bytes(body, sets),
        _ => {}
    }
}

fn required_sets(root: &Node) -> Vec<FirstBytes> {
    let mut sets = Vec::new();
    required_bytes(root, &mut sets);
    sets.retain(|bits| count_bits(bits) <= REQUIRED_SET_BYTES_MAX);
    sets.sort_by_key(count_bits);
    sets.dedup();
    sets.truncate(REQUIRED_SETS_MAX);
    sets.into_iter().map(|bits| FirstBytes { bits }).collect()
}

fn word_first_bytes(negated: bool) -> ByteBits {
    static WORD: OnceLock<[ByteBits; 2]> = OnceLock::new();
    let both = WORD.get_or_init(|| {
        let mut set = CodePointSet::from_ranges(unicode::WORD);
        for code in super::machine::LATIN1_NUMBER_WORDS {
            set.add(code);
        }
        let mut word = [0_u64; 4];
        add_set_bytes(&mut word, set.ranges());
        let mut not_word = [0_u64; 4];
        add_set_bytes(&mut not_word, set.complement().ranges());
        [word, not_word]
    });
    both[usize::from(negated)]
}
