use std::fmt;

const PROGRAM_INSTRUCTIONS_MAX: usize = 65_536;
const NEST_DEPTH_MAX: usize = 250;
const BUILD_ERROR: &str = "error building NFA";
const POSIX_CLASSES: [(&str, &[(char, char)]); 14] = [
    ("alnum", &[('0', '9'), ('A', 'Z'), ('a', 'z')]),
    ("alpha", &[('A', 'Z'), ('a', 'z')]),
    ("ascii", &[('\0', '\x7f')]),
    ("blank", &[('\t', '\t'), (' ', ' ')]),
    ("cntrl", &[('\0', '\x1f'), ('\x7f', '\x7f')]),
    ("digit", &[('0', '9')]),
    ("graph", &[('!', '~')]),
    ("lower", &[('a', 'z')]),
    ("print", &[(' ', '~')]),
    ("punct", &[('!', '/'), (':', '@'), ('[', '`'), ('{', '~')]),
    ("space", &[('\t', '\r'), (' ', ' ')]),
    ("upper", &[('A', 'Z')]),
    ("word", &[('0', '9'), ('A', 'Z'), ('_', '_'), ('a', 'z')]),
    ("xdigit", &[('0', '9'), ('A', 'F'), ('a', 'f')]),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternError {
    pattern: String,
    reason: &'static str,
}

impl fmt::Display for PatternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(BUILD_ERROR)
    }
}

impl std::error::Error for PatternError {}

#[derive(Clone, Debug)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class { ranges: Vec<(char, char)>, negated: bool },
    Start,
    End,
    Concat(Vec<Self>),
    Alternate(Vec<Self>),
    Repeat {
        node: Box<Self>,
        minimum: u32,
        maximum: Option<u32>,
        greedy: bool,
    },
}

#[derive(Clone, Debug)]
enum Instruction {
    Char(char),
    Any,
    Class { ranges: Vec<(char, char)>, negated: bool },
    Start,
    End,
    Split(usize, usize),
    Jump(usize),
    Match,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    program: Vec<Instruction>,
}

struct Parser<'a> {
    pattern: &'a str,
    chars: Vec<char>,
    position: usize,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, reason: &'static str) -> PatternError {
        PatternError {
            pattern: self.pattern.to_owned(),
            reason,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn next(&mut self) -> Option<char> {
        let next = self.peek()?;
        self.position += 1;
        Some(next)
    }

    fn alternation(&mut self) -> Result<Node, PatternError> {
        self.depth += 1;
        if self.depth > NEST_DEPTH_MAX {
            return Err(self.error("groups nested deeper than 250"));
        }
        let mut branches = vec![self.concatenation()?];
        while self.peek() == Some('|') {
            self.position += 1;
            branches.push(self.concatenation()?);
        }
        self.depth -= 1;
        Ok(if branches.len() == 1 {
            branches.remove(0)
        } else {
            Node::Alternate(branches)
        })
    }

    fn concatenation(&mut self) -> Result<Node, PatternError> {
        let mut items = Vec::new();
        while let Some(next) = self.peek() {
            if next == '|' || next == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.repetition(atom)?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.remove(0),
            _ => Node::Concat(items),
        })
    }

    fn atom(&mut self) -> Result<Node, PatternError> {
        let next = self.next().ok_or_else(|| self.error("unexpected end"))?;
        match next {
            '.' => Ok(Node::Any),
            '^' => Ok(Node::Start),
            '$' => Ok(Node::End),
            '(' => self.group(),
            '[' => self.class(),
            '\\' if self.peek() == Some('A') => {
                self.position += 1;
                Ok(Node::Start)
            }
            '\\' if self.peek() == Some('z') => {
                self.position += 1;
                Ok(Node::End)
            }
            '\\' => self.escape().map(Node::Char),
            '*' | '+' | '?' | '{' => Err(self.error("repetition operator missing expression")),
            literal => Ok(Node::Char(literal)),
        }
    }

    fn group(&mut self) -> Result<Node, PatternError> {
        if self.peek() == Some('?') {
            self.position += 1;
            match self.next() {
                Some(':') => {}
                Some('P') if self.peek() == Some('<') => {
                    self.position += 1;
                    self.group_name()?;
                }
                Some('<') => self.group_name()?,
                _ => return Err(self.error("flags are not supported")),
            }
        }
        let inner = self.alternation()?;
        if self.next() != Some(')') {
            return Err(self.error("unclosed group"));
        }
        Ok(inner)
    }

    fn group_name(&mut self) -> Result<(), PatternError> {
        let mut empty = true;
        loop {
            match self.next() {
                Some('>') if !empty => return Ok(()),
                Some(name) if name == '_' || name.is_ascii_alphanumeric() => empty = false,
                _ => return Err(self.error("invalid capture group name")),
            }
        }
    }

    fn escape(&mut self) -> Result<char, PatternError> {
        let next = self.next().ok_or_else(|| self.error("incomplete escape"))?;
        match next {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
            | '#' | '&' | '-' | '~' => Ok(next),
            'n' => Ok('\n'),
            't' => Ok('\t'),
            'r' => Ok('\r'),
            'f' => Ok('\u{c}'),
            'v' => Ok('\u{b}'),
            'a' => Ok('\u{7}'),
            'x' => self.hexadecimal(),
            _ => Err(self.error("escape classes and assertions are not supported")),
        }
    }

    fn hexadecimal(&mut self) -> Result<char, PatternError> {
        let mut digits = String::new();
        if self.peek() == Some('{') {
            self.position += 1;
            loop {
                match self.next() {
                    Some('}') if !digits.is_empty() => break,
                    Some(digit) if digit.is_ascii_hexdigit() => digits.push(digit),
                    _ => return Err(self.error("invalid hexadecimal escape")),
                }
            }
        } else {
            for _ in 0..2 {
                match self.next() {
                    Some(digit) if digit.is_ascii_hexdigit() => digits.push(digit),
                    _ => return Err(self.error("invalid hexadecimal escape")),
                }
            }
        }
        u32::from_str_radix(&digits, 16)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| self.error("invalid hexadecimal escape"))
    }

    fn posix_class(&mut self) -> Result<&'static [(char, char)], PatternError> {
        let start = self.position;
        while self.peek().is_some_and(|letter| letter.is_ascii_lowercase()) {
            self.position += 1;
        }
        let name: String = self.chars[start..self.position].iter().collect();
        if self.next() != Some(':') || self.next() != Some(']') {
            return Err(self.error("invalid POSIX class"));
        }
        POSIX_CLASSES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, ranges)| *ranges)
            .ok_or_else(|| self.error("unknown POSIX class"))
    }

    fn nested_class(&mut self, ranges: &mut Vec<(char, char)>) -> Result<(), PatternError> {
        if self.peek() == Some(':') {
            self.position += 1;
            ranges.extend_from_slice(self.posix_class()?);
            return Ok(());
        }
        match self.class()? {
            Node::Class {
                ranges: nested,
                negated: false,
            } => {
                ranges.extend(nested);
                Ok(())
            }
            _ => Err(self.error("negated nested classes are not supported")),
        }
    }

    fn class_member(&mut self, next: char) -> Result<char, PatternError> {
        match next {
            '\\' => self.escape(),
            '[' => Err(self.error("a nested class cannot bound a range")),
            '&' | '~' | '-' if self.peek() == Some(next) => {
                Err(self.error("class set operations are not supported"))
            }
            literal => Ok(literal),
        }
    }

    fn class(&mut self) -> Result<Node, PatternError> {
        let negated = self.peek() == Some('^');
        if negated {
            self.position += 1;
        }
        let mut ranges = Vec::new();
        let mut first = true;
        loop {
            let next = self.next().ok_or_else(|| self.error("unclosed class"))?;
            if next == ']' && !first {
                break;
            }
            first = false;
            if next == '[' {
                self.nested_class(&mut ranges)?;
                continue;
            }
            let low = self.class_member(next)?;
            let ranged = self.peek() == Some('-')
                && self
                    .chars
                    .get(self.position + 1)
                    .is_some_and(|after| *after != ']');
            let high = if ranged {
                self.position += 1;
                let next = self.next().ok_or_else(|| self.error("unclosed class"))?;
                self.class_member(next)?
            } else {
                low
            };
            if high < low {
                return Err(self.error("invalid class range"));
            }
            ranges.push((low, high));
        }
        Ok(Node::Class { ranges, negated })
    }

    fn repetition(&mut self, atom: Node) -> Result<Node, PatternError> {
        let (minimum, maximum) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => return self.counted(atom),
            _ => return Ok(atom),
        };
        self.position += 1;
        self.finish_repetition(atom, minimum, maximum)
    }

    fn counted(&mut self, atom: Node) -> Result<Node, PatternError> {
        self.position += 1;
        let minimum = self.number()?;
        let maximum = match self.next() {
            Some('}') => Some(minimum),
            Some(',') if self.peek() == Some('}') => {
                self.position += 1;
                None
            }
            Some(',') => {
                let maximum = self.number()?;
                if self.next() != Some('}') {
                    return Err(self.error("invalid counted repetition"));
                }
                Some(maximum)
            }
            _ => return Err(self.error("invalid counted repetition")),
        };
        if maximum.is_some_and(|maximum| maximum < minimum) {
            return Err(self.error("invalid counted repetition range"));
        }
        self.finish_repetition(atom, minimum, maximum)
    }

    fn number(&mut self) -> Result<u32, PatternError> {
        let start = self.position;
        while self.peek().is_some_and(|digit| digit.is_ascii_digit()) {
            self.position += 1;
        }
        let digits: String = self.chars[start..self.position].iter().collect();
        digits
            .parse()
            .map_err(|_| self.error("invalid counted repetition"))
    }

    fn finish_repetition(
        &mut self,
        atom: Node,
        minimum: u32,
        maximum: Option<u32>,
    ) -> Result<Node, PatternError> {
        let greedy = self.peek() != Some('?');
        if !greedy {
            self.position += 1;
        }
        if matches!(self.peek(), Some('*' | '+' | '?' | '{')) {
            return Err(self.error("nested repetition operators are not supported"));
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            minimum,
            maximum,
            greedy,
        })
    }
}

struct Compiler<'a> {
    program: Vec<Instruction>,
    pattern: &'a str,
}

impl Compiler<'_> {
    fn push(&mut self, instruction: Instruction) -> Result<usize, PatternError> {
        if self.program.len() >= PROGRAM_INSTRUCTIONS_MAX {
            return Err(PatternError {
                pattern: self.pattern.to_owned(),
                reason: "pattern needs more than 65,536 instructions",
            });
        }
        self.program.push(instruction);
        Ok(self.program.len() - 1)
    }

    fn set_split(&mut self, at: usize, greedy: bool, body: usize, skip: usize) {
        self.program[at] = if greedy {
            Instruction::Split(body, skip)
        } else {
            Instruction::Split(skip, body)
        };
    }

    fn compile(&mut self, node: &Node) -> Result<(), PatternError> {
        match node {
            Node::Empty => {}
            Node::Char(literal) => {
                self.push(Instruction::Char(*literal))?;
            }
            Node::Any => {
                self.push(Instruction::Any)?;
            }
            Node::Class { ranges, negated } => {
                self.push(Instruction::Class {
                    ranges: ranges.clone(),
                    negated: *negated,
                })?;
            }
            Node::Start => {
                self.push(Instruction::Start)?;
            }
            Node::End => {
                self.push(Instruction::End)?;
            }
            Node::Concat(items) => {
                for item in items {
                    self.compile(item)?;
                }
            }
            Node::Alternate(branches) => self.alternate(branches)?,
            Node::Repeat {
                node,
                minimum,
                maximum,
                greedy,
            } => self.repeat(node, *minimum, *maximum, *greedy)?,
        }
        Ok(())
    }

    fn alternate(&mut self, branches: &[Node]) -> Result<(), PatternError> {
        let mut jumps = Vec::new();
        let last = branches.len() - 1;
        for branch in &branches[..last] {
            let split = self.push(Instruction::Split(0, 0))?;
            self.compile(branch)?;
            jumps.push(self.push(Instruction::Jump(0))?);
            let next = self.program.len();
            self.program[split] = Instruction::Split(split + 1, next);
        }
        self.compile(&branches[last])?;
        let end = self.program.len();
        for jump in jumps {
            self.program[jump] = Instruction::Jump(end);
        }
        Ok(())
    }

    fn repeat(
        &mut self,
        node: &Node,
        minimum: u32,
        maximum: Option<u32>,
        greedy: bool,
    ) -> Result<(), PatternError> {
        let Some(maximum) = maximum else {
            return self.unbounded(node, minimum, greedy);
        };
        for _ in 0..minimum {
            self.compile(node)?;
        }
        let mut splits = Vec::new();
        for _ in minimum..maximum {
            splits.push(self.push(Instruction::Split(0, 0))?);
            self.compile(node)?;
        }
        let skip = self.program.len();
        for split in splits {
            self.set_split(split, greedy, split + 1, skip);
        }
        Ok(())
    }

    fn unbounded(&mut self, node: &Node, minimum: u32, greedy: bool) -> Result<(), PatternError> {
        if minimum == 0 && !matches_empty(node) {
            let split = self.push(Instruction::Split(0, 0))?;
            self.compile(node)?;
            self.push(Instruction::Jump(split))?;
            let skip = self.program.len();
            self.set_split(split, greedy, split + 1, skip);
            return Ok(());
        }
        let question = (minimum == 0)
            .then(|| self.push(Instruction::Split(0, 0)))
            .transpose()?;
        for _ in 1..minimum {
            self.compile(node)?;
        }
        let start = self.program.len();
        self.compile(node)?;
        let plus = self.push(Instruction::Split(0, 0))?;
        let end = self.program.len();
        self.set_split(plus, greedy, start, end);
        if let Some(question) = question {
            self.set_split(question, greedy, start, end);
        }
        Ok(())
    }
}

fn matches_empty(node: &Node) -> bool {
    match node {
        Node::Empty | Node::Start | Node::End => true,
        Node::Char(_) | Node::Any | Node::Class { .. } => false,
        Node::Concat(items) => items.iter().all(matches_empty),
        Node::Alternate(branches) => branches.iter().any(matches_empty),
        Node::Repeat { node, minimum, .. } => *minimum == 0 || matches_empty(node),
    }
}

impl Pattern {
    pub fn new(pattern: &str) -> Result<Self, PatternError> {
        let mut parser = Parser {
            pattern,
            chars: pattern.chars().collect(),
            position: 0,
            depth: 0,
        };
        let node = parser.alternation()?;
        if parser.position != parser.chars.len() {
            return Err(parser.error("unopened group"));
        }
        let mut compiler = Compiler {
            program: Vec::new(),
            pattern,
        };
        compiler.compile(&node)?;
        compiler.push(Instruction::Match)?;
        Ok(Self {
            program: compiler.program,
        })
    }

    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        let mut run = Run::new(self);
        for character in text.chars() {
            if !run.step(character) {
                return false;
            }
        }
        run.finish()
    }

    #[must_use]
    pub fn debug_matches(&self, value: &dyn fmt::Debug) -> bool {
        use fmt::Write as _;
        let mut run = Run::new(self);
        let mut writer = RunWriter {
            run: &mut run,
            dead: false,
        };
        write!(writer, "{value:?}").expect("the pattern writer never fails");
        if writer.dead {
            return false;
        }
        run.finish()
    }
}

struct RunWriter<'a, 'b> {
    run: &'a mut Run<'b>,
    dead: bool,
}

impl fmt::Write for RunWriter<'_, '_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for character in text.chars() {
            if self.dead {
                break;
            }
            self.dead = !self.run.step(character);
        }
        Ok(())
    }
}

struct Run<'a> {
    pattern: &'a Pattern,
    threads: Vec<usize>,
    seen: Vec<bool>,
    stack: Vec<usize>,
    matched: bool,
    matched_here: bool,
    position: usize,
}

struct Closure {
    start: bool,
    at_start: bool,
    at_end: bool,
}

impl<'a> Run<'a> {
    fn new(pattern: &'a Pattern) -> Self {
        let size = pattern.program.len();
        let mut run = Self {
            pattern,
            threads: Vec::with_capacity(size),
            seen: vec![false; size],
            stack: Vec::with_capacity(size),
            matched: false,
            matched_here: false,
            position: 0,
        };
        let (threads, found) = run.closure(
            &[],
            &Closure {
                start: true,
                at_start: true,
                at_end: false,
            },
        );
        run.threads = threads;
        run.matched_here = found;
        run
    }

    fn closure(&mut self, from: &[usize], closure: &Closure) -> (Vec<usize>, bool) {
        self.seen.iter_mut().for_each(|seen| *seen = false);
        let mut threads = Vec::new();
        let starts = from
            .iter()
            .copied()
            .chain((closure.start && !self.matched).then_some(0));
        for start in starts {
            self.stack.clear();
            self.stack.push(start);
            while let Some(pc) = self.stack.pop() {
                if self.seen[pc] {
                    continue;
                }
                self.seen[pc] = true;
                match &self.pattern.program[pc] {
                    Instruction::Split(first, second) => {
                        self.stack.push(*second);
                        self.stack.push(*first);
                    }
                    Instruction::Jump(target) => self.stack.push(*target),
                    Instruction::Start => {
                        if closure.at_start {
                            self.stack.push(pc + 1);
                        }
                    }
                    Instruction::End if closure.at_end => self.stack.push(pc + 1),
                    Instruction::Match => {
                        self.matched = true;
                        return (threads, true);
                    }
                    Instruction::End
                    | Instruction::Char(_)
                    | Instruction::Any
                    | Instruction::Class { .. } => threads.push(pc),
                }
            }
        }
        (threads, false)
    }

    fn accepts(&self, pc: usize, character: char) -> bool {
        match &self.pattern.program[pc] {
            Instruction::Char(literal) => *literal == character,
            Instruction::Any => character != '\n',
            Instruction::Class { ranges, negated } => {
                ranges
                    .iter()
                    .any(|(low, high)| (*low..=*high).contains(&character))
                    != *negated
            }
            Instruction::End => false,
            Instruction::Start
            | Instruction::Split(..)
            | Instruction::Jump(_)
            | Instruction::Match => {
                unreachable!("only consuming instructions and end assertions wait for input")
            }
        }
    }

    fn step(&mut self, character: char) -> bool {
        let next: Vec<usize> = self
            .threads
            .iter()
            .copied()
            .filter(|pc| self.accepts(*pc, character))
            .map(|pc| pc + 1)
            .collect();
        self.position += 1;
        let (threads, found) = self.closure(
            &next,
            &Closure {
                start: true,
                at_start: false,
                at_end: false,
            },
        );
        self.threads = threads;
        self.matched_here = found;
        !self.threads.is_empty() || found || !self.matched
    }

    fn finish(mut self) -> bool {
        if self.matched_here {
            return true;
        }
        let waiting: Vec<usize> = self
            .threads
            .iter()
            .copied()
            .filter(|pc| matches!(self.pattern.program[*pc], Instruction::End))
            .map(|pc| pc + 1)
            .collect();
        let at_start = self.position == 0;
        self.closure(
            &waiting,
            &Closure {
                start: false,
                at_start,
                at_end: true,
            },
        )
        .1
    }
}

