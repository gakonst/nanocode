mod class;
mod compile;
mod fold;
mod machine;
mod parse;
mod tune;
mod unicode;

use class::CodePointSet;
use std::fmt;

pub(crate) use machine::Searcher;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    ParseDepthLimit,
    NumberTooBig,
    InvalidCodePoint,
    TooLongWideCharValue,
    EndPatternInGroup,
    EndPatternAtEscape,
    EndPatternAtMeta,
    EndPatternAtControl,
    EndPatternWithUnmatchedParenthesis,
    UnmatchedCloseParenthesis,
    MetaCodeSyntax,
    ControlCodeSyntax,
    RepeatRangeTooBig,
    RepeatTargetMissing,
    RepeatTargetInvalid,
    InvalidGroupName,
    EmptyGroupName,
    InvalidBackref,
    UndefinedNameReference,
    EmptyCharClass,
    PrematureEndOfCharClass,
    EmptyRangeInCharClass,
    CharClassValueAtEndOfRange,
    UnmatchedRangeSpecifierInCharClass,
    InvalidPosixBracketType,
    InvalidCharPropertyName,
    InvalidLookBehindPattern,
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(what) => write!(formatter, "unsupported regular expression {what}"),
            other => write!(formatter, "invalid regular expression: {other:?}"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchError {
    RetryLimitInMatch,
}

impl fmt::Display for SearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RetryLimitInMatch => write!(
                formatter,
                "a regular expression search backtracked more than {} times at one position",
                machine::RETRY_LIMIT_IN_MATCH
            ),
        }
    }
}

impl std::error::Error for SearchError {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Anchor {
    BeginLine,
    EndLine,
    BeginBuffer,
    EndBuffer,
    SemiEndBuffer,
    BeginPosition,
    WordBoundary,
    NotWordBoundary,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CharType {
    Word,
    Space,
    Digit,
    HexDigit,
}

#[derive(Clone, Debug)]
struct Class {
    set: CodePointSet,
    negated: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Emptiness {
    NotEmpty,
    MayBeEmpty,
}

#[derive(Clone, Debug)]
struct Repeat {
    lower: u32,
    upper: Option<u32>,
    greedy: bool,
    emptiness: Emptiness,
    head_exact: Option<u8>,
    next_head_exact: Option<u8>,
    body: Box<Node>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LookBehindLength {
    Unknown,
    Fixed(u32),
}

#[derive(Clone, Debug)]
enum Node {
    Empty,
    Fail,
    String {
        text: String,
        ignore_case: bool,
        expanded: bool,
    },
    Class(Class),
    Word {
        negated: bool,
    },
    Any {
        multiline: bool,
    },
    Anchor(Anchor),
    LookAhead {
        negated: bool,
        body: Box<Self>,
    },
    LookBehind {
        negated: bool,
        length: LookBehindLength,
        body: Box<Self>,
    },
    Backref {
        group: usize,
        ignore_case: bool,
    },
    Capture {
        group: usize,
        body: Box<Self>,
    },
    Atomic(Box<Self>),
    Options(Box<Self>),
    Repeat(Repeat),
    Concat(Vec<Self>),
    Alternation(Vec<Self>),
}

pub(crate) struct Regex {
    program: compile::Program,
}

impl Regex {
    pub(crate) fn new(pattern: &str) -> Result<Self, Error> {
        let parsed = parse::parse(pattern)?;
        let root = tune::tune(parsed.root, parsed.groups)?;
        let program = compile::compile(&root, parsed.groups)?;
        Ok(Self { program })
    }

    pub(crate) fn search(
        &self,
        text: &str,
        start: usize,
        region: &mut Region,
        searcher: &mut Searcher,
    ) -> Result<bool, SearchError> {
        assert!(start <= text.len(), "a search starts inside its text");
        assert!(
            text.is_char_boundary(start),
            "a search starts at a character boundary"
        );
        searcher.search(&self.program, text, start, region)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Region {
    positions: Vec<Option<(usize, usize)>>,
}

impl Region {
    pub(crate) fn pos(&self, index: usize) -> Option<(usize, usize)> {
        self.positions.get(index).copied().flatten()
    }
}
