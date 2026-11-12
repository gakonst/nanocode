use super::{Class, Emptiness, Error, LookBehindLength, Node, Repeat, fold};

const EXPAND_STRING_LENGTH_MAX: usize = 100;
const LOOK_BEHIND_CHARACTERS_MAX: u32 = i32::MAX as u32;
const INFINITE: u32 = u32::MAX;

#[derive(Clone, Copy, Default)]
struct State {
    in_look_behind: bool,
}

struct Context {
    groups: usize,
    referenced: Vec<bool>,
}

pub(super) fn tune(root: Node, groups: usize) -> Result<Node, Error> {
    let mut root = reduce_strings(root);
    let mut referenced = vec![false; groups + 1];
    collect_backrefs(&root, &mut referenced)?;
    let context = Context { groups, referenced };
    tune_node(&mut root, State::default(), &context)?;
    Ok(root)
}

fn collect_backrefs(node: &Node, referenced: &mut [bool]) -> Result<(), Error> {
    match node {
        Node::Backref { group, .. } => {
            let Some(slot) = referenced.get_mut(*group) else {
                return Err(Error::InvalidBackref);
            };
            *slot = true;
        }
        Node::LookAhead { body, .. }
        | Node::LookBehind { body, .. }
        | Node::Capture { body, .. }
        | Node::Atomic(body)
        | Node::Options(body) => collect_backrefs(body, referenced)?,
        Node::Repeat(repeat) => collect_backrefs(&repeat.body, referenced)?,
        Node::Concat(nodes) | Node::Alternation(nodes) => {
            for node in nodes {
                collect_backrefs(node, referenced)?;
            }
        }
        Node::Empty
        | Node::Fail
        | Node::String { .. }
        | Node::Class(_)
        | Node::Word { .. }
        | Node::Any { .. }
        | Node::Anchor(_) => {}
    }
    Ok(())
}

fn reduce_strings(node: Node) -> Node {
    match node {
        Node::Concat(nodes) => {
            let mut reduced: Vec<Node> = Vec::with_capacity(nodes.len());
            for node in nodes {
                let node = reduce_strings(node);
                if let (
                    Some(Node::String {
                        text: previous,
                        ignore_case: previous_ignore_case,
                        expanded: previous_expanded,
                    }),
                    Node::String {
                        text,
                        ignore_case,
                        expanded,
                    },
                ) = (reduced.last_mut(), &node)
                    && previous_ignore_case == ignore_case
                    && previous_expanded == expanded
                {
                    previous.push_str(text);
                    continue;
                }
                reduced.push(node);
            }
            Node::Concat(reduced)
        }
        Node::Alternation(nodes) => {
            Node::Alternation(nodes.into_iter().map(reduce_strings).collect())
        }
        Node::LookAhead { negated, body } => Node::LookAhead {
            negated,
            body: Box::new(reduce_strings(*body)),
        },
        Node::LookBehind {
            negated,
            length,
            body,
        } => Node::LookBehind {
            negated,
            length,
            body: Box::new(reduce_strings(*body)),
        },
        Node::Capture { group, body } => Node::Capture {
            group,
            body: Box::new(reduce_strings(*body)),
        },
        Node::Atomic(body) => Node::Atomic(Box::new(reduce_strings(*body))),
        Node::Options(body) => Node::Options(Box::new(reduce_strings(*body))),
        Node::Repeat(repeat) => Node::Repeat(Repeat {
            body: Box::new(reduce_strings(*repeat.body)),
            ..repeat
        }),
        other => other,
    }
}

fn tune_node(node: &mut Node, state: State, context: &Context) -> Result<(), Error> {
    match node {
        Node::Concat(nodes) => {
            for index in 0..nodes.len() {
                tune_node(&mut nodes[index], state, context)?;
                if index > 0 {
                    let (before, after) = nodes.split_at_mut(index);
                    tune_next(&mut before[index - 1], &after[0]);
                }
            }
        }
        Node::Alternation(nodes) => {
            for node in nodes {
                tune_node(node, state, context)?;
            }
        }
        Node::String {
            text,
            ignore_case: true,
            expanded: false,
        } => {
            if !text.is_empty() {
                *node = fold::unravel(text, state.in_look_behind)?;
            }
        }
        Node::Backref { group, .. } => {
            if *group > context.groups {
                return Err(Error::InvalidBackref);
            }
        }
        Node::Capture { body, .. } | Node::Atomic(body) | Node::Options(body) => {
            tune_node(body, state, context)?;
        }
        Node::LookAhead { body, .. } => tune_node(body, state, context)?,
        Node::Repeat(_) => tune_repeat(node, state, context)?,
        Node::LookBehind { .. } => tune_look_behind(node, state, context)?,
        Node::Empty
        | Node::Fail
        | Node::String { .. }
        | Node::Class(_)
        | Node::Word { .. }
        | Node::Any { .. }
        | Node::Anchor(_) => {}
    }
    Ok(())
}

fn tune_repeat(node: &mut Node, state: State, context: &Context) -> Result<(), Error> {
    let Node::Repeat(repeat) = node else {
        unreachable!("tune_repeat is called on a repeat")
    };
    if repeat.upper.is_none_or(|upper| upper >= 1) && min_byte_length(&repeat.body) == 0 {
        repeat.emptiness = Emptiness::MayBeEmpty;
    }
    tune_node(&mut repeat.body, state, context)?;
    if repeat.greedy && repeat.emptiness == Emptiness::NotEmpty {
        if let Node::Repeat(inner) = repeat.body.as_mut() {
            if inner.head_exact.is_some() {
                repeat.head_exact = inner.head_exact.take();
            }
        } else {
            repeat.head_exact = exact_head_byte(&repeat.body);
        }
    }
    if let Node::String {
        text,
        ignore_case,
        expanded,
    } = repeat.body.as_ref()
        && repeat.upper == Some(repeat.lower)
        && repeat.lower > 1
        && repeat.lower as usize <= EXPAND_STRING_LENGTH_MAX
        && text.len() * repeat.lower as usize <= EXPAND_STRING_LENGTH_MAX
    {
        *node = Node::String {
            text: text.repeat(repeat.lower as usize),
            ignore_case: *ignore_case,
            expanded: *expanded,
        };
    }
    Ok(())
}

pub(super) fn min_byte_length(node: &Node) -> usize {
    match node {
        Node::Empty
        | Node::Fail
        | Node::Anchor(_)
        | Node::LookAhead { .. }
        | Node::LookBehind { .. } => 0,
        Node::Backref { .. } => 0,
        Node::String { text, .. } => text.len(),
        Node::Class(_) | Node::Word { .. } | Node::Any { .. } => 1,
        Node::Capture { body, .. } | Node::Atomic(body) | Node::Options(body) => {
            min_byte_length(body)
        }
        Node::Repeat(repeat) => {
            if repeat.lower == 0 {
                0
            } else {
                min_byte_length(&repeat.body).saturating_mul(repeat.lower as usize)
            }
        }
        Node::Concat(nodes) => nodes
            .iter()
            .map(min_byte_length)
            .fold(0, usize::saturating_add),
        Node::Alternation(nodes) => nodes.iter().map(min_byte_length).min().unwrap_or(0),
    }
}

fn check_look_behind(
    node: &Node,
    negated: bool,
    used: &mut bool,
    context: &Context,
) -> Result<(), Error> {
    match node {
        Node::Concat(nodes) | Node::Alternation(nodes) => {
            for node in nodes {
                check_look_behind(node, negated, used, context)?;
            }
            Ok(())
        }
        Node::Repeat(repeat) => check_look_behind(&repeat.body, negated, used, context),
        Node::Capture { group, body } => {
            if negated {
                return Err(Error::InvalidLookBehindPattern);
            }
            check_look_behind(body, negated, used, context)?;
            if context.referenced[*group] {
                *used = true;
            }
            Ok(())
        }
        Node::Options(body) | Node::Atomic(body) => check_look_behind(body, negated, used, context),
        Node::Anchor(anchor) => match anchor {
            super::Anchor::EndBuffer | super::Anchor::SemiEndBuffer => {
                Err(Error::InvalidLookBehindPattern)
            }
            _ => Ok(()),
        },
        Node::LookAhead { .. } => Err(Error::InvalidLookBehindPattern),
        Node::LookBehind {
            negated: inner,
            body,
            ..
        } => {
            if *inner && !negated {
                return Err(Error::InvalidLookBehindPattern);
            }
            check_look_behind(body, negated, used, context)
        }
        Node::Backref { .. } => Err(Error::Unsupported("backreference in look-behind")),
        Node::Empty
        | Node::Fail
        | Node::String { .. }
        | Node::Class(_)
        | Node::Word { .. }
        | Node::Any { .. } => Ok(()),
    }
}

fn reduce_one(node: &mut Node) -> bool {
    let Node::Repeat(repeat) = node else {
        return false;
    };
    if matches!(
        repeat.body.as_ref(),
        Node::String { .. }
            | Node::Word { .. }
            | Node::Any { .. }
            | Node::Class(_)
            | Node::Backref { .. }
    ) {
        repeat.upper = Some(repeat.lower);
        return repeat.lower == 0;
    }
    false
}

fn reduce_list(node: &mut Node) -> bool {
    match node {
        Node::Repeat(_) => {
            reduce_one(node);
            false
        }
        Node::Concat(nodes) => {
            let mut removed = false;
            for node in nodes.iter_mut() {
                removed = reduce_one(node);
                if !removed {
                    break;
                }
            }
            removed
        }
        _ => false,
    }
}

fn reduce_look_behind(body: &mut Node) -> Result<(), Error> {
    let removed = match body {
        Node::Alternation(nodes) => {
            let mut removed = false;
            for node in nodes.iter_mut() {
                removed = reduce_list(node);
                if removed {
                    break;
                }
            }
            removed
        }
        other => reduce_list(other),
    };
    if removed {
        return Err(Error::InvalidLookBehindPattern);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct CharLength {
    min: u32,
    max: u32,
    min_is_sure: bool,
}

impl CharLength {
    const fn exact(length: u32) -> Self {
        Self {
            min: length,
            max: length,
            min_is_sure: true,
        }
    }

    const fn fixed(self) -> bool {
        self.min == self.max && self.min != INFINITE
    }

    fn add(&mut self, other: Self) {
        self.min = distance_add(self.min, other.min);
        self.max = distance_add(self.max, other.max);
        self.min_is_sure = other.min_is_sure && self.min_is_sure;
    }

    const fn alternative(&mut self, other: Self) {
        if self.min > other.min {
            self.min = other.min;
            self.min_is_sure = other.min_is_sure;
        } else if self.min == other.min && other.min_is_sure {
            self.min_is_sure = true;
        }
        if self.max < other.max {
            self.max = other.max;
        }
    }
}

fn distance_add(left: u32, right: u32) -> u32 {
    if left == INFINITE || right == INFINITE {
        INFINITE
    } else {
        left.checked_add(right)
            .filter(|sum| *sum != INFINITE)
            .unwrap_or(INFINITE)
    }
}

const fn distance_multiply(length: u32, count: u32) -> u32 {
    if count == 0 {
        return 0;
    }
    if length < INFINITE / count {
        length * count
    } else {
        INFINITE
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Normal,
    TopAlternativesFixed,
}

fn char_length(node: &Node, level: u32) -> Result<(CharLength, Shape), Error> {
    let level = level + 1;
    let length = match node {
        Node::Concat(nodes) => {
            let mut total: Option<CharLength> = None;
            for node in nodes {
                let (length, _) = char_length(node, level)?;
                match &mut total {
                    None => total = Some(length),
                    Some(total) => total.add(length),
                }
            }
            total.expect("a concatenation has nodes")
        }
        Node::Alternation(nodes) => {
            let (mut length, _) = char_length(&nodes[0], level)?;
            let mut fixed = true;
            for node in &nodes[1..] {
                let (other, _) = char_length(node, level)?;
                if !other.fixed() {
                    fixed = false;
                }
                length.alternative(other);
            }
            if !length.fixed() && fixed && level == 1 {
                return Ok((length, Shape::TopAlternativesFixed));
            }
            length
        }
        Node::String { text, .. } => CharLength::exact(text.chars().count() as u32),
        Node::Empty => CharLength::exact(0),
        Node::Repeat(repeat) => {
            if repeat.upper == Some(repeat.lower) {
                if repeat.lower == 0 {
                    CharLength::exact(0)
                } else {
                    let (mut length, _) = char_length(&repeat.body, level)?;
                    length.min = distance_multiply(length.min, repeat.lower);
                    length.max = distance_multiply(length.max, repeat.lower);
                    length
                }
            } else {
                let (mut length, _) = char_length(&repeat.body, level)?;
                length.min = distance_multiply(length.min, repeat.lower);
                length.max = match repeat.upper {
                    None => INFINITE,
                    Some(upper) => distance_multiply(length.max, upper),
                };
                length
            }
        }
        Node::Class(_) | Node::Word { .. } | Node::Any { .. } => CharLength::exact(1),
        Node::Capture { body, .. } => {
            let (mut length, _) = char_length(body, level)?;
            length.min_is_sure = false;
            length
        }
        Node::Options(body) | Node::Atomic(body) => char_length(body, level)?.0,
        Node::Fail => CharLength::exact(0),
        Node::Anchor(_) | Node::LookAhead { .. } | Node::LookBehind { .. } => CharLength {
            min: 0,
            max: 0,
            min_is_sure: false,
        },
        Node::Backref { .. } => return Err(Error::Unsupported("backreference in look-behind")),
    };
    Ok((length, Shape::Normal))
}

fn tune_look_behind(node: &mut Node, state: State, context: &Context) -> Result<(), Error> {
    let Node::LookBehind { negated, body, .. } = node else {
        unreachable!("tune_look_behind is called on a look-behind")
    };
    let negated = *negated;
    let mut used = false;
    check_look_behind(body, negated, &mut used, context)?;
    tune_node(
        body,
        State {
            in_look_behind: true,
        },
        context,
    )?;
    reduce_look_behind(body)?;
    let (length, shape) = char_length(body, 0)?;
    if (length.max != INFINITE && length.max > LOOK_BEHIND_CHARACTERS_MAX)
        || length.min > LOOK_BEHIND_CHARACTERS_MAX
    {
        return Err(Error::InvalidLookBehindPattern);
    }
    if length.min == 0 && length.min_is_sure && !used {
        *node = if negated { Node::Fail } else { Node::Empty };
        return Ok(());
    }
    match shape {
        Shape::TopAlternativesFixed => {
            let Node::Alternation(branches) = std::mem::replace(body.as_mut(), Node::Empty) else {
                unreachable!("top alternatives come from an alternation")
            };
            let parts: Vec<Node> = branches
                .into_iter()
                .map(|branch| Node::LookBehind {
                    negated,
                    length: LookBehindLength::Unknown,
                    body: Box::new(branch),
                })
                .collect();
            *node = if negated {
                Node::Concat(parts)
            } else {
                Node::Alternation(parts)
            };
            tune_node(node, state, context)
        }
        Shape::Normal => {
            if length.min == INFINITE {
                return Err(Error::InvalidLookBehindPattern);
            }
            if length.min != length.max {
                return Err(Error::Unsupported("variable length look-behind"));
            }
            let Node::LookBehind { length: slot, .. } = node else {
                unreachable!("still a look-behind")
            };
            *slot = LookBehindLength::Fixed(length.min);
            Ok(())
        }
    }
}

fn head_literal(node: &Node) -> Option<&Node> {
    match node {
        Node::Word { .. } | Node::Class(_) => Some(node),
        Node::String { text, .. } if !text.is_empty() => Some(node),
        Node::Concat(nodes) => nodes.first().and_then(head_literal),
        Node::Repeat(repeat) if repeat.lower > 0 => head_literal(&repeat.body),
        Node::Options(body) | Node::Capture { body, .. } | Node::Atomic(body) => head_literal(body),
        Node::LookAhead {
            negated: false,
            body,
        } => head_literal(body),
        _ => None,
    }
}

const SINGLE_BYTE_SIZE: u32 = 0x100;
const ASCII_SIZE: u32 = 0x80;

fn class_byte(class: &Class, byte: u32) -> bool {
    let raw = byte < ASCII_SIZE && class.set.contains(byte);
    raw != class.negated
}

fn class_has_multibyte(class: &Class) -> bool {
    class
        .set
        .ranges()
        .last()
        .is_some_and(|&(_, last)| last >= ASCII_SIZE)
}

fn first_char(text: &str) -> char {
    text.chars()
        .next()
        .expect("head literal strings are not empty")
}

fn exclusive(x: &Node, y: &Node) -> bool {
    match (x, y) {
        (Node::Word { negated: x_negated }, Node::Word { negated: y_negated }) => {
            x_negated != y_negated
        }
        (Node::Word { .. }, Node::Class(_) | Node::String { .. }) => exclusive(y, x),
        (Node::Class(class), Node::Word { negated }) => {
            if !negated {
                if class_has_multibyte(class) || class.negated {
                    return false;
                }
                !(0..SINGLE_BYTE_SIZE).any(|code| class_byte(class, code) && word_code(code))
            } else {
                if class_has_multibyte(class) || class.negated {
                    return false;
                }
                !(0..SINGLE_BYTE_SIZE).any(|code| !word_code(code) && class_byte(class, code))
            }
        }
        (Node::Class(x_class), Node::Class(y_class)) => {
            if (0..SINGLE_BYTE_SIZE)
                .any(|code| class_byte(x_class, code) && class_byte(y_class, code))
            {
                return false;
            }
            (!class_has_multibyte(x_class) && !x_class.negated)
                || (!class_has_multibyte(y_class) && !y_class.negated)
        }
        (Node::Class(_), Node::String { .. }) => exclusive(y, x),
        (Node::String { text, .. }, Node::Word { negated }) => {
            if word_code(first_char(text) as u32) {
                *negated
            } else {
                !negated
            }
        }
        (Node::String { text, .. }, Node::Class(class)) => {
            let code = first_char(text) as u32;
            class.set.contains(code) == class.negated
        }
        (Node::String { text: x_text, .. }, Node::String { text: y_text, .. }) => {
            x_text.bytes().zip(y_text.bytes()).any(|(a, b)| a != b)
        }
        _ => false,
    }
}

fn word_code(code: u32) -> bool {
    char::from_u32(code).is_some_and(super::machine::is_word)
}

const fn strict_real(node: &Node) -> bool {
    match node {
        Node::String { text, .. } => !text.is_empty(),
        Node::Class(_) | Node::Word { .. } | Node::Any { .. } => true,
        _ => false,
    }
}

fn tune_next(node: &mut Node, next: &Node) {
    match node {
        Node::Repeat(repeat) => {
            if !(repeat.greedy && repeat.upper.is_none()) {
                return;
            }
            if let Some(byte) = exact_head_byte(next)
                && byte != 0
            {
                repeat.next_head_exact = Some(byte);
            }
            if !(repeat.lower <= 1 && strict_real(&repeat.body)) {
                return;
            }
            let Some(x) = head_literal(&repeat.body) else {
                return;
            };
            let Some(y) = head_literal(next) else {
                return;
            };
            if exclusive(x, y) {
                let repeat = std::mem::replace(node, Node::Empty);
                *node = Node::Atomic(Box::new(repeat));
            }
        }
        Node::Capture { body, .. } => tune_next(body, next),
        _ => {}
    }
}

fn exact_head_byte(node: &Node) -> Option<u8> {
    match node {
        Node::String { text, .. } => text.as_bytes().first().copied(),
        Node::Concat(nodes) => nodes.first().and_then(exact_head_byte),
        Node::Repeat(repeat) if repeat.lower > 0 => {
            repeat.head_exact.or_else(|| exact_head_byte(&repeat.body))
        }
        Node::Options(body) | Node::Capture { body, .. } | Node::Atomic(body) => {
            exact_head_byte(body)
        }
        Node::LookAhead {
            negated: false,
            body,
        } => exact_head_byte(body),
        _ => None,
    }
}
