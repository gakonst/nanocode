use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use crate::ast::Span;

const DIVIDER_WIDTH: usize = 79;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    CaptureLimitExceeded,
    ClassEscapeInvalid,
    ClassRangeInvalid,
    ClassRangeLiteral,
    ClassUnclosed,
    DecimalEmpty,
    DecimalInvalid,
    EscapeHexEmpty,
    EscapeHexInvalid,
    EscapeHexInvalidDigit,
    EscapeUnexpectedEof,
    EscapeUnrecognized,
    FlagDanglingNegation,
    FlagDuplicate { original: Span },
    FlagRepeatedNegation { original: Span },
    FlagUnexpectedEof,
    FlagUnrecognized,
    GroupNameDuplicate { original: Span },
    GroupNameEmpty,
    GroupNameInvalid,
    GroupNameUnexpectedEof,
    GroupUnclosed,
    GroupUnopened,
    NestLimitExceeded(u32),
    RepetitionCountInvalid,
    RepetitionCountDecimalEmpty,
    RepetitionCountUnclosed,
    RepetitionMissing,
    SpecialWordBoundaryUnclosed,
    SpecialWordBoundaryUnrecognized,
    SpecialWordOrRepetitionUnexpectedEof,
    UnicodeClassInvalid,
    UnsupportedBackreference,
    UnsupportedLookAround,
    UnicodeNotAllowed,
    InvalidUtf8,
    InvalidLineTerminator,
    UnicodePropertyNotFound,
    UnicodePropertyValueNotFound,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ErrorKind::*;
        match self {
            CaptureLimitExceeded => write!(f, "exceeded the maximum number of capturing groups ({})", u32::MAX),
            ClassEscapeInvalid => f.write_str("invalid escape sequence found in character class"),
            ClassRangeInvalid => f.write_str("invalid character class range, the start must be <= the end"),
            ClassRangeLiteral => f.write_str("invalid range boundary, must be a literal"),
            ClassUnclosed => f.write_str("unclosed character class"),
            DecimalEmpty => f.write_str("decimal literal empty"),
            DecimalInvalid => f.write_str("decimal literal invalid"),
            EscapeHexEmpty => f.write_str("hexadecimal literal empty"),
            EscapeHexInvalid => f.write_str("hexadecimal literal is not a Unicode scalar value"),
            EscapeHexInvalidDigit => f.write_str("invalid hexadecimal digit"),
            EscapeUnexpectedEof => f.write_str("incomplete escape sequence, reached end of pattern prematurely"),
            EscapeUnrecognized => f.write_str("unrecognized escape sequence"),
            FlagDanglingNegation => f.write_str("dangling flag negation operator"),
            FlagDuplicate { .. } => f.write_str("duplicate flag"),
            FlagRepeatedNegation { .. } => f.write_str("flag negation operator repeated"),
            FlagUnexpectedEof => f.write_str("expected flag but got end of regex"),
            FlagUnrecognized => f.write_str("unrecognized flag"),
            GroupNameDuplicate { .. } => f.write_str("duplicate capture group name"),
            GroupNameEmpty => f.write_str("empty capture group name"),
            GroupNameInvalid => f.write_str("invalid capture group character"),
            GroupNameUnexpectedEof => f.write_str("unclosed capture group name"),
            GroupUnclosed => f.write_str("unclosed group"),
            GroupUnopened => f.write_str("unopened group"),
            NestLimitExceeded(limit) => {
                write!(f, "exceed the maximum number of nested parentheses/brackets ({limit})")
            }
            RepetitionCountInvalid => f.write_str("invalid repetition count range, the start must be <= the end"),
            RepetitionCountDecimalEmpty => f.write_str("repetition quantifier expects a valid decimal"),
            RepetitionCountUnclosed => f.write_str("unclosed counted repetition"),
            RepetitionMissing => f.write_str("repetition operator missing expression"),
            SpecialWordBoundaryUnclosed => f.write_str(
                "special word boundary assertion is either unclosed or contains an invalid character",
            ),
            SpecialWordBoundaryUnrecognized => f.write_str(
                "unrecognized special word boundary assertion, valid choices are: start, end, start-half or end-half",
            ),
            SpecialWordOrRepetitionUnexpectedEof => f.write_str(
                "found either the beginning of a special word boundary or a bounded repetition on a \\b with an opening brace, but no closing brace",
            ),
            UnicodeClassInvalid => f.write_str("invalid Unicode character class"),
            UnsupportedBackreference => f.write_str("backreferences are not supported"),
            UnsupportedLookAround => {
                f.write_str("look-around, including look-ahead and look-behind, is not supported")
            }
            UnicodeNotAllowed => f.write_str("Unicode not allowed here"),
            InvalidUtf8 => f.write_str("pattern can match invalid UTF-8"),
            InvalidLineTerminator => f.write_str("invalid line terminator, must be ASCII"),
            UnicodePropertyNotFound => f.write_str("Unicode property not found"),
            UnicodePropertyValueNotFound => f.write_str("Unicode property value not found"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Inner {
    kind: ErrorKind,
    pattern: String,
    span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    inner: Box<Inner>,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, pattern: &str, span: Span) -> Error {
        Error {
            inner: Box::new(Inner {
                kind,
                pattern: pattern.to_string(),
                span,
            }),
        }
    }

    pub fn kind(&self) -> &ErrorKind {
        &self.inner.kind
    }

    pub fn span(&self) -> &Span {
        &self.inner.span
    }

    pub(crate) fn specialize(mut self, from: ErrorKind, to: ErrorKind) -> Error {
        if self.inner.kind == from {
            self.inner.kind = to;
        }
        self
    }

    fn auxiliary_span(&self) -> Option<Span> {
        match self.inner.kind {
            ErrorKind::FlagDuplicate { original }
            | ErrorKind::FlagRepeatedNegation { original }
            | ErrorKind::GroupNameDuplicate { original } => Some(original),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let spans = Spans::new(&self.inner.pattern, self.inner.span, self.auxiliary_span());
        if self.inner.pattern.contains('\n') {
            let divider = "~".repeat(DIVIDER_WIDTH);
            writeln!(f, "regex parse error:")?;
            writeln!(f, "{divider}")?;
            write!(f, "{}", spans.notate())?;
            writeln!(f, "{divider}")?;
            if !spans.multi_line.is_empty() {
                let mut notes = Vec::new();
                for span in &spans.multi_line {
                    notes.push(format!(
                        "on line {} (column {}) through line {} (column {})",
                        span.start.line,
                        span.start.column,
                        span.end.line,
                        span.end.column - 1
                    ));
                }
                writeln!(f, "{}", notes.join("\n"))?;
            }
            write!(f, "error: {}", self.inner.kind)
        } else {
            writeln!(f, "regex parse error:")?;
            write!(f, "{}", spans.notate())?;
            write!(f, "error: {}", self.inner.kind)
        }
    }
}

struct Spans<'p> {
    pattern: &'p str,
    line_number_width: usize,
    by_line: Vec<Vec<Span>>,
    multi_line: Vec<Span>,
}

impl<'p> Spans<'p> {
    fn new(pattern: &'p str, span: Span, auxiliary: Option<Span>) -> Spans<'p> {
        let mut line_count = pattern.lines().count();
        if pattern.ends_with('\n') {
            line_count += 1;
        }
        let line_number_width = if line_count <= 1 {
            0
        } else {
            line_count.to_string().len()
        };
        let mut spans = Spans {
            pattern,
            line_number_width,
            by_line: vec![Vec::new(); line_count],
            multi_line: Vec::new(),
        };
        spans.add(span);
        if let Some(span) = auxiliary {
            spans.add(span);
        }
        spans
    }

    fn add(&mut self, span: Span) {
        if span.is_one_line() {
            let line = &mut self.by_line[span.start.line - 1];
            line.push(span);
            line.sort_by_key(Span::key);
        } else {
            self.multi_line.push(span);
            self.multi_line.sort_by_key(Span::key);
        }
    }

    fn notate(&self) -> String {
        let mut notated = String::new();
        for (index, line) in self.pattern.lines().enumerate() {
            if self.line_number_width > 0 {
                notated.push_str(&self.left_pad_line_number(index + 1));
                notated.push_str(": ");
            } else {
                notated.push_str("    ");
            }
            notated.push_str(line);
            notated.push('\n');
            if let Some(notes) = self.notate_line(index) {
                notated.push_str(&notes);
                notated.push('\n');
            }
        }
        notated
    }

    fn notate_line(&self, index: usize) -> Option<String> {
        let spans = &self.by_line[index];
        if spans.is_empty() {
            return None;
        }
        let mut notes = " ".repeat(self.line_number_padding());
        let mut position = 0;
        for span in spans {
            let padding = (span.start.column - 1).saturating_sub(position);
            notes.extend(core::iter::repeat_n(' ', padding));
            position += padding;
            let length = span.end.column.saturating_sub(span.start.column);
            for _ in 0..core::cmp::max(1, length) {
                notes.push('^');
                position += 1;
            }
        }
        Some(notes)
    }

    fn left_pad_line_number(&self, number: usize) -> String {
        let number = number.to_string();
        let pad = self
            .line_number_width
            .checked_sub(number.len())
            .expect("line numbers fit their width");
        let mut out = " ".repeat(pad);
        out.push_str(&number);
        out
    }

    fn line_number_padding(&self) -> usize {
        if self.line_number_width == 0 {
            4
        } else {
            2 + self.line_number_width
        }
    }
}
