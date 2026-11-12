use std::ops::Range;

use super::parse::{Node, Parsed};
use super::set::CodePointSet;

#[derive(Clone, Copy, Debug)]
pub(super) struct LoopFields {
    pub(super) id: usize,
    pub(super) min: usize,
    pub(super) max: usize,
    pub(super) greedy: bool,
    pub(super) exit: usize,
}

#[derive(Debug)]
pub(super) enum Instruction {
    Char(u32),
    CharSet(Box<[u32]>),
    Bracket {
        invert: bool,
        ascii: u128,
        set: CodePointSet,
    },
    MatchAny,
    MatchAnyExceptLineTerminator,
    StartOfLine {
        multiline: bool,
    },
    EndOfLine {
        multiline: bool,
    },
    WordBoundary {
        invert: bool,
        ignore_case: bool,
    },
    Jump(usize),
    Alt(usize),
    BeginGroup(usize),
    EndGroup(usize),
    ResetGroup(usize),
    BackReference {
        group: usize,
        ignore_case: bool,
    },
    Lookaround {
        negate: bool,
        backwards: bool,
        groups: Range<usize>,
        continuation: usize,
    },
    EnterLoop(LoopFields),
    LoopAgain {
        begin: usize,
    },
    LoopOneCharacter {
        min: usize,
        max: usize,
        greedy: bool,
    },
    Goal,
    Fail,
}

const BYTE_VALUES: usize = 256;
const UTF8_SINGLE_BYTE_LIMIT: u32 = 0x80;
const UTF8_DOUBLE_BYTE_LIMIT: u32 = 0x800;
const UTF8_TRIPLE_BYTE_LIMIT: u32 = 0x1_0000;

#[derive(Debug)]
pub(super) enum Start {
    Anywhere,
    Anchored,
    Sequence(Box<[u8]>),
    AnyByte(Box<[u8]>),
    Bytes(Box<[bool; BYTE_VALUES]>),
}

const SCANNED_BYTES_MAX: usize = 3;

enum Prefix {
    Arbitrary,
    Sequence(Vec<u8>),
    Bytes(Box<[bool; BYTE_VALUES]>),
}

#[derive(Debug)]
pub(super) struct Program {
    pub(super) instructions: Vec<Instruction>,
    pub(super) loop_count: usize,
    pub(super) group_count: usize,
    pub(super) start: Start,
}

enum Pending<'a> {
    Node(&'a Node),
    AlternativeNext {
        nodes: &'a [Node],
        index: usize,
        alt: usize,
        jumps: Vec<usize>,
    },
    AlternativesFinish {
        jumps: Vec<usize>,
    },
    LoopFinish {
        begin: usize,
    },
    LookaroundFinish {
        instruction: usize,
    },
    EndGroup(usize),
}

pub(super) fn compile(mut parsed: Parsed) -> Program {
    simplify(&mut parsed.node);
    let mut emitter = Emitter {
        instructions: Vec::new(),
        loop_count: 0,
    };
    emitter.emit(&parsed.node);
    emitter.instructions.push(Instruction::Goal);
    Program {
        instructions: emitter.instructions,
        loop_count: emitter.loop_count,
        group_count: usize::from(parsed.group_count),
        start: start_of(&parsed.node),
    }
}

fn start_of(node: &Node) -> Start {
    if is_start_anchored(node) {
        return Start::Anchored;
    }
    match prefix(node) {
        Some(Prefix::Sequence(bytes)) if !bytes.is_empty() => {
            Start::Sequence(bytes.into_boxed_slice())
        }
        Some(Prefix::Bytes(bytes)) if bytes.iter().any(|&set| set) => {
            let listed: Vec<u8> = (0..=u8::MAX)
                .filter(|&byte| bytes[usize::from(byte)])
                .collect();
            if listed.len() <= SCANNED_BYTES_MAX {
                Start::AnyByte(listed.into_boxed_slice())
            } else {
                Start::Bytes(bytes)
            }
        }
        _ => Start::Anywhere,
    }
}

fn is_start_anchored(node: &Node) -> bool {
    match node {
        Node::StartOfLine { multiline } => !multiline,
        Node::Cat(nodes) => nodes.first().is_some_and(is_start_anchored),
        Node::Group { contents, .. } => is_start_anchored(contents),
        Node::Alt(nodes) => nodes.iter().all(is_start_anchored),
        _ => false,
    }
}

const fn first_byte(code_point: u32) -> u8 {
    let [_, high, middle, low] = code_point.to_be_bytes();
    if code_point < UTF8_SINGLE_BYTE_LIMIT {
        low
    } else if code_point < UTF8_DOUBLE_BYTE_LIMIT {
        0b1100_0000 | (((code_point >> 6) as u8) & 0x1F)
    } else if code_point < UTF8_TRIPLE_BYTE_LIMIT {
        0b1110_0000 | (middle >> 4)
    } else {
        0b1111_0000 | ((high >> 2) & 0x07)
    }
}

fn byte_set(code_points: impl IntoIterator<Item = u32>) -> Prefix {
    let mut bytes = Box::new([false; BYTE_VALUES]);
    for code_point in code_points {
        bytes[usize::from(first_byte(code_point))] = true;
    }
    Prefix::Bytes(bytes)
}

fn set_first_bytes(set: &CodePointSet) -> Prefix {
    let mut bytes = Box::new([false; BYTE_VALUES]);
    let bounds = [
        (0, UTF8_SINGLE_BYTE_LIMIT - 1),
        (UTF8_SINGLE_BYTE_LIMIT, UTF8_DOUBLE_BYTE_LIMIT - 1),
        (UTF8_DOUBLE_BYTE_LIMIT, UTF8_TRIPLE_BYTE_LIMIT - 1),
        (UTF8_TRIPLE_BYTE_LIMIT, super::set::CODE_POINT_MAX),
    ];
    for interval in set.intervals() {
        for (low, high) in bounds {
            let first = interval.first.max(low);
            let last = interval.last.min(high);
            if first <= last {
                for byte in first_byte(first)..=first_byte(last) {
                    bytes[usize::from(byte)] = true;
                }
            }
        }
    }
    Prefix::Bytes(bytes)
}

const fn literal(node: &Node) -> Option<char> {
    match node {
        Node::Char(c) => char::from_u32(*c),
        _ => None,
    }
}

fn prefix(node: &Node) -> Option<Prefix> {
    match node {
        Node::Char(c) => Some(match char::from_u32(*c) {
            Some(c) => Prefix::Sequence(c.to_string().into_bytes()),
            None => Prefix::Arbitrary,
        }),
        Node::CharSet(chars) => Some(byte_set(chars.iter().copied())),
        Node::Bracket { invert, set } => Some(if *invert {
            set_first_bytes(&set.inverted())
        } else {
            set_first_bytes(set)
        }),
        Node::Cat(nodes) => sequence_prefix(nodes),
        Node::Group { contents, .. } => prefix(contents),
        Node::Lookaround { .. } => None,
        Node::Loop {
            body, quantifier, ..
        } => {
            if quantifier.min > 0 {
                prefix(body)
            } else {
                Some(Prefix::Arbitrary)
            }
        }
        Node::Alt(nodes) => alternatives_prefix(nodes),
        _ => Some(Prefix::Arbitrary),
    }
}

fn sequence_prefix(nodes: &[Node]) -> Option<Prefix> {
    let index = nodes.iter().position(|node| prefix(node).is_some())?;
    if literal(&nodes[index]).is_none() {
        return prefix(&nodes[index]);
    }
    let text: String = nodes[index..].iter().map_while(literal).collect();
    Some(Prefix::Sequence(text.into_bytes()))
}

fn alternatives_prefix(nodes: &[Node]) -> Option<Prefix> {
    let (last, rest) = nodes.split_last()?;
    let mut combined = prefix(last);
    for node in rest.iter().rev() {
        combined = match (prefix(node), combined) {
            (Some(left), Some(right)) => Some(disjunction(left, right)),
            _ => Some(Prefix::Arbitrary),
        };
    }
    combined
}

fn disjunction(left: Prefix, right: Prefix) -> Prefix {
    match (left, right) {
        (Prefix::Arbitrary, _) | (_, Prefix::Arbitrary) => Prefix::Arbitrary,
        (Prefix::Sequence(left), Prefix::Sequence(right)) => {
            let shared = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
            if shared > 0 {
                Prefix::Sequence(left[..shared].to_vec())
            } else {
                let mut bytes = Box::new([false; BYTE_VALUES]);
                bytes[usize::from(left[0])] = true;
                bytes[usize::from(right[0])] = true;
                Prefix::Bytes(bytes)
            }
        }
        (Prefix::Bytes(mut left), Prefix::Bytes(right)) => {
            for (slot, set) in left.iter_mut().zip(right.iter()) {
                *slot |= *set;
            }
            Prefix::Bytes(left)
        }
        (Prefix::Bytes(mut bytes), Prefix::Sequence(sequence))
        | (Prefix::Sequence(sequence), Prefix::Bytes(mut bytes)) => {
            bytes[usize::from(sequence[0])] = true;
            Prefix::Bytes(bytes)
        }
    }
}

fn bracket_is_empty(invert: bool, set: &CodePointSet) -> bool {
    if invert {
        set.contains_all()
    } else {
        set.is_empty()
    }
}

fn contains_groups(node: &Node) -> bool {
    match node {
        Node::Group { .. } => true,
        Node::Cat(nodes) | Node::Alt(nodes) => nodes.iter().any(contains_groups),
        Node::Loop { body, .. } => contains_groups(body),
        Node::Lookaround { contents, .. } => contains_groups(contents),
        _ => false,
    }
}

fn single_character(node: &Node) -> bool {
    match node {
        Node::Char(_) | Node::MatchAny | Node::MatchAnyExceptLineTerminator => true,
        Node::CharSet(set) => !set.is_empty(),
        Node::Bracket { invert, set } => !bracket_is_empty(*invert, set),
        _ => false,
    }
}

fn fails(node: &Node) -> bool {
    match node {
        Node::CharSet(set) => set.is_empty(),
        Node::Bracket { invert, set } => bracket_is_empty(*invert, set),
        _ => false,
    }
}

fn simplify(node: &mut Node) {
    match node {
        Node::Bracket { invert, set } if bracket_is_empty(*invert, set) => {
            *node = Node::CharSet(Vec::new());
        }
        Node::Cat(nodes) => {
            nodes.iter_mut().for_each(simplify);
            let failing = nodes.iter().any(fails) && !nodes.iter().any(contains_groups);
            nodes.retain(|child| !matches!(child, Node::Empty));
            *node = if failing {
                Node::CharSet(Vec::new())
            } else {
                match nodes.len() {
                    0 => Node::Empty,
                    1 => nodes.pop().expect("one node"),
                    _ => return,
                }
            };
        }
        Node::Alt(nodes) => {
            nodes.iter_mut().for_each(simplify);
            let mut chain = fold_alternatives(std::mem::take(nodes));
            *node = if chain.len() == 1 {
                chain.pop().expect("one alternative")
            } else {
                Node::Alt(chain)
            };
        }
        Node::Group { contents, .. } => simplify(contents),
        Node::Lookaround {
            negate, contents, ..
        } => {
            simplify(contents);
            if !*negate && matches!(**contents, Node::Empty) {
                *node = Node::Empty;
            }
        }
        Node::Loop {
            body,
            quantifier,
            groups,
        } => {
            let removable =
                quantifier.max == Some(0) && groups.start == groups.end && !single_character(body);
            simplify(body);
            if removable || matches!(**body, Node::Empty) {
                *node = Node::Empty;
            } else if groups.start == groups.end && quantifier.min > 0 && fails(body) {
                *node = Node::CharSet(Vec::new());
            }
        }
        _ => {}
    }
}

fn fold_alternatives(mut nodes: Vec<Node>) -> Vec<Node> {
    let last = nodes.pop().expect("an alternation has alternatives");
    let mut chain = vec![last];
    while let Some(alternative) = nodes.pop() {
        let groups = contains_groups(&alternative) || chain.iter().any(contains_groups);
        let rest_fails = chain.len() == 1 && fails(&chain[0]);
        if !groups && fails(&alternative) {
            if rest_fails {
                chain = vec![Node::CharSet(Vec::new())];
            }
            continue;
        }
        if !groups && rest_fails {
            chain = vec![alternative];
            continue;
        }
        if matches!(alternative, Node::Empty) && matches!(chain.as_slice(), [Node::Empty]) {
            continue;
        }
        chain.insert(0, alternative);
    }
    chain
}

struct Emitter {
    instructions: Vec<Instruction>,
    loop_count: usize,
}

impl Emitter {
    fn push(&mut self, instruction: Instruction) -> usize {
        self.instructions.push(instruction);
        self.instructions.len() - 1
    }

    const fn next(&self) -> usize {
        self.instructions.len()
    }

    fn emit(&mut self, root: &Node) {
        let mut stack = vec![Pending::Node(root)];
        while let Some(pending) = stack.pop() {
            match pending {
                Pending::Node(node) => self.emit_node(node, &mut stack),
                Pending::AlternativeNext {
                    nodes,
                    index,
                    alt,
                    jumps,
                } => self.emit_alternative(nodes, index, alt, jumps, &mut stack),
                Pending::AlternativesFinish { jumps } => {
                    let end = self.next();
                    for jump in jumps {
                        self.instructions[jump] = Instruction::Jump(end);
                    }
                }
                Pending::LoopFinish { begin } => {
                    self.push(Instruction::LoopAgain { begin });
                    let exit = self.next();
                    let Instruction::EnterLoop(fields) = &mut self.instructions[begin] else {
                        unreachable!("a loop begins with EnterLoop");
                    };
                    fields.exit = exit;
                }
                Pending::LookaroundFinish { instruction } => {
                    self.push(Instruction::Goal);
                    let next = self.next();
                    let Instruction::Lookaround { continuation, .. } =
                        &mut self.instructions[instruction]
                    else {
                        unreachable!("a lookaround begins with Lookaround");
                    };
                    *continuation = next;
                }
                Pending::EndGroup(id) => {
                    self.push(Instruction::EndGroup(id));
                }
            }
        }
    }

    fn emit_alt<'a>(&mut self, nodes: &'a [Node], stack: &mut Vec<Pending<'a>>) {
        assert!(nodes.len() >= 2);
        let alt = self.push(Instruction::Alt(0));
        stack.push(Pending::AlternativeNext {
            nodes,
            index: 1,
            alt,
            jumps: Vec::with_capacity(nodes.len() - 1),
        });
        stack.push(Pending::Node(&nodes[0]));
    }

    fn emit_alternative<'a>(
        &mut self,
        nodes: &'a [Node],
        index: usize,
        alt: usize,
        mut jumps: Vec<usize>,
        stack: &mut Vec<Pending<'a>>,
    ) {
        jumps.push(self.push(Instruction::Jump(0)));
        let secondary = self.next();
        self.instructions[alt] = Instruction::Alt(secondary);
        if index + 1 == nodes.len() {
            stack.push(Pending::AlternativesFinish { jumps });
        } else {
            let next_alt = self.push(Instruction::Alt(0));
            stack.push(Pending::AlternativeNext {
                nodes,
                index: index + 1,
                alt: next_alt,
                jumps,
            });
        }
        stack.push(Pending::Node(&nodes[index]));
    }

    fn emit_node<'a>(&mut self, node: &'a Node, stack: &mut Vec<Pending<'a>>) {
        let instruction = match node {
            Node::Empty => return,
            Node::Char(c) => Instruction::Char(*c),
            Node::CharSet(set) if set.is_empty() => Instruction::Fail,
            Node::CharSet(set) => Instruction::CharSet(set.clone().into_boxed_slice()),
            Node::Bracket { invert, set } => Instruction::Bracket {
                invert: *invert,
                ascii: set.ascii_mask(),
                set: set.clone(),
            },
            Node::MatchAny => Instruction::MatchAny,
            Node::MatchAnyExceptLineTerminator => Instruction::MatchAnyExceptLineTerminator,
            Node::StartOfLine { multiline } => Instruction::StartOfLine {
                multiline: *multiline,
            },
            Node::EndOfLine { multiline } => Instruction::EndOfLine {
                multiline: *multiline,
            },
            Node::WordBoundary {
                invert,
                ignore_case,
            } => Instruction::WordBoundary {
                invert: *invert,
                ignore_case: *ignore_case,
            },
            Node::BackReference { group, ignore_case } => {
                assert!(*group >= 1);
                Instruction::BackReference {
                    group: *group as usize - 1,
                    ignore_case: *ignore_case,
                }
            }
            Node::Cat(nodes) => {
                stack.extend(nodes.iter().rev().map(Pending::Node));
                return;
            }
            Node::Alt(nodes) => {
                self.emit_alt(nodes, stack);
                return;
            }
            Node::Group { id, contents } => {
                let id = usize::from(*id);
                self.push(Instruction::BeginGroup(id));
                stack.push(Pending::EndGroup(id));
                stack.push(Pending::Node(contents));
                return;
            }
            Node::Lookaround {
                negate,
                backwards,
                groups,
                contents,
            } => {
                let instruction = self.push(Instruction::Lookaround {
                    negate: *negate,
                    backwards: *backwards,
                    groups: usize::from(groups.start)..usize::from(groups.end),
                    continuation: 0,
                });
                stack.push(Pending::LookaroundFinish { instruction });
                stack.push(Pending::Node(contents));
                return;
            }
            Node::Loop {
                body,
                quantifier,
                groups,
            } => {
                let max = quantifier.max.unwrap_or(usize::MAX);
                if groups.is_empty() && single_character(body) {
                    if matches!(**body, Node::Char(c) if char::from_u32(c).is_none()) {
                        self.push(Instruction::Fail);
                        return;
                    }
                    self.push(Instruction::LoopOneCharacter {
                        min: quantifier.min,
                        max,
                        greedy: quantifier.greedy,
                    });
                    stack.push(Pending::Node(body));
                    return;
                }
                let begin = self.push(Instruction::EnterLoop(LoopFields {
                    id: self.loop_count,
                    min: quantifier.min,
                    max,
                    greedy: quantifier.greedy,
                    exit: 0,
                }));
                self.loop_count += 1;
                for group in groups.clone() {
                    self.push(Instruction::ResetGroup(usize::from(group)));
                }
                stack.push(Pending::LoopFinish { begin });
                stack.push(Pending::Node(body));
                return;
            }
        };
        self.push(instruction);
    }
}
