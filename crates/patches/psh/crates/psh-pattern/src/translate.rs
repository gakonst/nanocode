use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::ast::{
    AsciiKind, AssertionKind, Ast, BinaryOpKind, ClassPerl, ClassUnicode as AstClassUnicode, Flag,
    FlagsItemKind, GroupKind, Literal, Node, NodeId, PerlKind, RepetitionKind, SetId, SetNode,
    Span, UnicodeKind,
};
use crate::class::{ClassBytes, ClassUnicode, Range};
use crate::error::{Error, ErrorKind};
use crate::hir::{Capture, Class, Dot, Hir, Look, Repetition};
use crate::unicode::{self, LookupError, Query};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Flags {
    pub case_insensitive: Option<bool>,
    pub multi_line: Option<bool>,
    pub dot_matches_new_line: Option<bool>,
    pub swap_greed: Option<bool>,
    pub unicode: Option<bool>,
    pub crlf: Option<bool>,
}

impl Flags {
    fn from_ast(flags: &crate::ast::Flags) -> Flags {
        let mut out = Flags::default();
        let mut enable = true;
        for item in &flags.items {
            match item.kind {
                FlagsItemKind::Negation => enable = false,
                FlagsItemKind::Flag(Flag::CaseInsensitive) => out.case_insensitive = Some(enable),
                FlagsItemKind::Flag(Flag::MultiLine) => out.multi_line = Some(enable),
                FlagsItemKind::Flag(Flag::DotMatchesNewLine) => {
                    out.dot_matches_new_line = Some(enable)
                }
                FlagsItemKind::Flag(Flag::SwapGreed) => out.swap_greed = Some(enable),
                FlagsItemKind::Flag(Flag::Unicode) => out.unicode = Some(enable),
                FlagsItemKind::Flag(Flag::Crlf) => out.crlf = Some(enable),
                FlagsItemKind::Flag(Flag::IgnoreWhitespace) => {}
            }
        }
        out
    }

    fn merge(&mut self, previous: &Flags) {
        self.case_insensitive = self.case_insensitive.or(previous.case_insensitive);
        self.multi_line = self.multi_line.or(previous.multi_line);
        self.dot_matches_new_line = self.dot_matches_new_line.or(previous.dot_matches_new_line);
        self.swap_greed = self.swap_greed.or(previous.swap_greed);
        self.unicode = self.unicode.or(previous.unicode);
        self.crlf = self.crlf.or(previous.crlf);
    }

    fn case_insensitive(&self) -> bool {
        self.case_insensitive.unwrap_or(false)
    }

    fn multi_line(&self) -> bool {
        self.multi_line.unwrap_or(false)
    }

    fn dot_matches_new_line(&self) -> bool {
        self.dot_matches_new_line.unwrap_or(false)
    }

    fn swap_greed(&self) -> bool {
        self.swap_greed.unwrap_or(false)
    }

    fn unicode(&self) -> bool {
        self.unicode.unwrap_or(true)
    }

    fn crlf(&self) -> bool {
        self.crlf.unwrap_or(false)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TranslateOptions {
    pub utf8: bool,
    pub line_terminator: u8,
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub dot_matches_new_line: bool,
    pub crlf: bool,
    pub swap_greed: bool,
    pub unicode: bool,
}

impl Default for TranslateOptions {
    fn default() -> TranslateOptions {
        TranslateOptions {
            utf8: true,
            line_terminator: b'\n',
            case_insensitive: false,
            multi_line: false,
            dot_matches_new_line: false,
            crlf: false,
            swap_greed: false,
            unicode: true,
        }
    }
}

pub fn translate(pattern: &str, ast: &Ast, options: TranslateOptions) -> Result<Hir, Error> {
    let flags = Flags {
        case_insensitive: options.case_insensitive.then_some(true),
        multi_line: options.multi_line.then_some(true),
        dot_matches_new_line: options.dot_matches_new_line.then_some(true),
        crlf: options.crlf.then_some(true),
        swap_greed: options.swap_greed.then_some(true),
        unicode: if options.unicode { None } else { Some(false) },
    };
    let mut translator = Translator {
        pattern,
        ast,
        flags,
        utf8: options.utf8,
        line_terminator: options.line_terminator,
    };
    translator.node(ast.root)
}

enum Accumulator {
    Unicode(ClassUnicode),
    Bytes(ClassBytes),
}

struct Translator<'a> {
    pattern: &'a str,
    ast: &'a Ast,
    flags: Flags,
    utf8: bool,
    line_terminator: u8,
}

impl<'a> Translator<'a> {
    fn error(&self, span: Span, kind: ErrorKind) -> Error {
        Error::new(kind, self.pattern, span)
    }

    fn set_flags(&mut self, flags: &crate::ast::Flags) -> Flags {
        let old = self.flags;
        let mut new = Flags::from_ast(flags);
        new.merge(&old);
        self.flags = new;
        old
    }

    fn node(&mut self, id: NodeId) -> Result<Hir, Error> {
        let ast = self.ast;
        match &ast.nodes[id] {
            Node::Empty(_) => Ok(Hir::empty()),
            Node::Flags { flags, .. } => {
                self.set_flags(flags);
                Ok(Hir::empty())
            }
            Node::Literal(literal) => self.literal(literal),
            Node::Dot(span) => self.dot(*span),
            Node::Assertion { kind, .. } => Ok(self.assertion(*kind)),
            Node::ClassPerl(class) => {
                if self.flags.unicode() {
                    Ok(Hir::class(Class::Unicode(self.perl_unicode(class))))
                } else {
                    Ok(Hir::class(Class::Bytes(self.perl_bytes(class)?)))
                }
            }
            Node::ClassUnicode(class) => Ok(Hir::class(Class::Unicode(self.unicode_class(class)?))),
            Node::ClassBracketed { span, negated, set } => {
                if self.flags.unicode() {
                    let mut accumulator = Accumulator::Unicode(ClassUnicode::empty());
                    self.set(*set, &mut accumulator)?;
                    let Accumulator::Unicode(mut class) = accumulator else {
                        unreachable!("unicode accumulator")
                    };
                    self.unicode_fold_and_negate(*negated, &mut class);
                    Ok(Hir::class(Class::Unicode(class)))
                } else {
                    let mut accumulator = Accumulator::Bytes(ClassBytes::empty());
                    self.set(*set, &mut accumulator)?;
                    let Accumulator::Bytes(mut class) = accumulator else {
                        unreachable!("byte accumulator")
                    };
                    self.bytes_fold_and_negate(*span, *negated, &mut class)?;
                    Ok(Hir::class(Class::Bytes(class)))
                }
            }
            Node::Repetition {
                kind,
                greedy,
                ast: sub,
                ..
            } => {
                let sub = self.node(*sub)?;
                let (min, max) = match *kind {
                    RepetitionKind::ZeroOrOne => (0, Some(1)),
                    RepetitionKind::ZeroOrMore => (0, None),
                    RepetitionKind::OneOrMore => (1, None),
                    RepetitionKind::Exactly(m) => (m, Some(m)),
                    RepetitionKind::AtLeast(m) => (m, None),
                    RepetitionKind::Bounded(m, n) => (m, Some(n)),
                };
                let greedy = if self.flags.swap_greed() {
                    !*greedy
                } else {
                    *greedy
                };
                Ok(Hir::repetition(Repetition {
                    min,
                    max,
                    greedy,
                    sub: Box::new(sub),
                }))
            }
            Node::Group { kind, ast: sub, .. } => {
                let old = match kind {
                    GroupKind::NonCapturing(flags) => self.set_flags(flags),
                    _ => self.flags,
                };
                let inner = self.node(*sub);
                self.flags = old;
                let inner = inner?;
                Ok(match kind {
                    GroupKind::CaptureIndex(index) => Hir::capture(Capture {
                        index: *index,
                        name: None,
                        sub: Box::new(inner),
                    }),
                    GroupKind::CaptureName { name, index } => Hir::capture(Capture {
                        index: *index,
                        name: Some(name.clone()),
                        sub: Box::new(inner),
                    }),
                    GroupKind::NonCapturing(_) => inner,
                })
            }
            Node::Concat { asts, .. } => {
                let mut subs = Vec::with_capacity(asts.len());
                for child in asts {
                    let hir = self.node(*child)?;
                    if !matches!(hir.kind(), crate::hir::HirKind::Empty) {
                        subs.push(hir);
                    }
                }
                Ok(Hir::concat(subs))
            }
            Node::Alternation { asts, .. } => {
                let mut subs = Vec::with_capacity(asts.len());
                for child in asts {
                    subs.push(self.node(*child)?);
                }
                Ok(Hir::alternation(subs))
            }
        }
    }

    fn literal_scalar(&self, literal: &Literal) -> Result<Result<char, u8>, Error> {
        if self.flags.unicode() {
            return Ok(Ok(literal.c));
        }
        let Some(byte) = literal.byte() else {
            return Ok(Ok(literal.c));
        };
        if byte <= 0x7F {
            return Ok(Ok(char::from(byte)));
        }
        if self.utf8 {
            return Err(self.error(literal.span, ErrorKind::InvalidUtf8));
        }
        Ok(Err(byte))
    }

    fn literal(&self, literal: &Literal) -> Result<Hir, Error> {
        match self.literal_scalar(literal)? {
            Err(byte) => Ok(Hir::literal(alloc::vec![byte])),
            Ok(c) => match self.case_fold_char(c) {
                Some(hir) => Ok(hir),
                None => {
                    let mut buffer = [0; 4];
                    Ok(Hir::literal(c.encode_utf8(&mut buffer).as_bytes().to_vec()))
                }
            },
        }
    }

    fn case_fold_char(&self, c: char) -> Option<Hir> {
        if !self.flags.case_insensitive() {
            return None;
        }
        let value = u32::from(c);
        if self.flags.unicode() {
            if !unicode::simple_fold_overlaps(value, value) {
                return None;
            }
            let mut class = ClassUnicode::new([Range {
                start: value,
                end: value,
            }]);
            class.case_fold_simple();
            return Some(Hir::class(Class::Unicode(class)));
        }
        if !c.is_ascii_alphabetic() {
            return None;
        }
        let mut class = ClassBytes::new([Range {
            start: value,
            end: value,
        }]);
        class.case_fold_simple();
        Some(Hir::class(Class::Bytes(class)))
    }

    fn dot(&self, span: Span) -> Result<Hir, Error> {
        let flags = self.flags;
        let terminator = self.line_terminator;
        if self.utf8 && (!flags.unicode() || !terminator.is_ascii()) {
            return Err(self.error(span, ErrorKind::InvalidUtf8));
        }
        let dot = if flags.dot_matches_new_line() {
            if flags.unicode() {
                Dot::AnyChar
            } else {
                Dot::AnyByte
            }
        } else if flags.unicode() {
            if flags.crlf() {
                Dot::AnyCharExceptCrlf
            } else {
                if !terminator.is_ascii() {
                    return Err(self.error(span, ErrorKind::InvalidLineTerminator));
                }
                Dot::AnyCharExcept(char::from(terminator))
            }
        } else if flags.crlf() {
            Dot::AnyByteExceptCrlf
        } else {
            Dot::AnyByteExcept(terminator)
        };
        Ok(Hir::dot(dot))
    }

    fn assertion(&self, kind: AssertionKind) -> Hir {
        let unicode = self.flags.unicode();
        let multi_line = self.flags.multi_line();
        let crlf = self.flags.crlf();
        let pick = |u: Look, a: Look| if unicode { u } else { a };
        Hir::look(match kind {
            AssertionKind::StartLine if multi_line => {
                if crlf {
                    Look::StartCrlf
                } else {
                    Look::StartLf
                }
            }
            AssertionKind::StartLine => Look::Start,
            AssertionKind::EndLine if multi_line => {
                if crlf {
                    Look::EndCrlf
                } else {
                    Look::EndLf
                }
            }
            AssertionKind::EndLine => Look::End,
            AssertionKind::StartText => Look::Start,
            AssertionKind::EndText => Look::End,
            AssertionKind::WordBoundary => pick(Look::WordUnicode, Look::WordAscii),
            AssertionKind::NotWordBoundary => pick(Look::WordUnicodeNegate, Look::WordAsciiNegate),
            AssertionKind::WordBoundaryStart | AssertionKind::WordBoundaryStartAngle => {
                pick(Look::WordStartUnicode, Look::WordStartAscii)
            }
            AssertionKind::WordBoundaryEnd | AssertionKind::WordBoundaryEndAngle => {
                pick(Look::WordEndUnicode, Look::WordEndAscii)
            }
            AssertionKind::WordBoundaryStartHalf => {
                pick(Look::WordStartHalfUnicode, Look::WordStartHalfAscii)
            }
            AssertionKind::WordBoundaryEndHalf => {
                pick(Look::WordEndHalfUnicode, Look::WordEndHalfAscii)
            }
        })
    }

    fn unicode_class(&self, class: &AstClassUnicode) -> Result<ClassUnicode, Error> {
        if !self.flags.unicode() {
            return Err(self.error(class.span, ErrorKind::UnicodeNotAllowed));
        }
        let (query, negated) = match &class.kind {
            UnicodeKind::OneLetter(c) => (Query::OneLetter(*c), class.negated),
            UnicodeKind::Named(name) => (Query::Binary(name), class.negated),
            UnicodeKind::NamedValue {
                name,
                value,
                not_equal,
            } => (
                Query::ByValue { name, value },
                if *not_equal {
                    !class.negated
                } else {
                    class.negated
                },
            ),
        };
        let mut out = unicode::class(query).map_err(|error| {
            self.error(
                class.span,
                match error {
                    LookupError::PropertyNotFound => ErrorKind::UnicodePropertyNotFound,
                    LookupError::PropertyValueNotFound => ErrorKind::UnicodePropertyValueNotFound,
                },
            )
        })?;
        self.unicode_fold_and_negate(negated, &mut out);
        Ok(out)
    }

    fn perl_unicode(&self, class: &ClassPerl) -> ClassUnicode {
        let mut out = match class.kind {
            PerlKind::Digit => unicode::perl_digit(),
            PerlKind::Space => unicode::perl_space(),
            PerlKind::Word => unicode::perl_word(),
        };
        if class.negated {
            out.negate();
        }
        out
    }

    fn perl_bytes(&self, class: &ClassPerl) -> Result<ClassBytes, Error> {
        let kind = match class.kind {
            PerlKind::Digit => AsciiKind::Digit,
            PerlKind::Space => AsciiKind::Space,
            PerlKind::Word => AsciiKind::Word,
        };
        let mut out = ClassBytes::new(ascii_ranges(kind));
        if class.negated {
            out.negate();
        }
        if self.utf8 && !out.is_ascii() {
            return Err(self.error(class.span, ErrorKind::InvalidUtf8));
        }
        Ok(out)
    }

    fn unicode_fold_and_negate(&self, negated: bool, class: &mut ClassUnicode) {
        if self.flags.case_insensitive() {
            class.case_fold_simple();
        }
        if negated {
            class.negate();
        }
    }

    fn bytes_fold_and_negate(
        &self,
        span: Span,
        negated: bool,
        class: &mut ClassBytes,
    ) -> Result<(), Error> {
        if self.flags.case_insensitive() {
            class.case_fold_simple();
        }
        if negated {
            class.negate();
        }
        if self.utf8 && !class.is_ascii() {
            return Err(self.error(span, ErrorKind::InvalidUtf8));
        }
        Ok(())
    }

    fn class_literal_byte(&self, literal: &Literal) -> Result<u32, Error> {
        match self.literal_scalar(literal)? {
            Err(byte) => Ok(u32::from(byte)),
            Ok(c) if c.is_ascii() => Ok(u32::from(c)),
            Ok(_) => Err(self.error(literal.span, ErrorKind::UnicodeNotAllowed)),
        }
    }

    fn set(&self, id: SetId, accumulator: &mut Accumulator) -> Result<(), Error> {
        let ast = self.ast;
        match &ast.sets[id] {
            SetNode::Empty(_) => {}
            SetNode::Literal(literal) => match accumulator {
                Accumulator::Unicode(class) => class.push(Range {
                    start: u32::from(literal.c),
                    end: u32::from(literal.c),
                }),
                Accumulator::Bytes(class) => {
                    let byte = self.class_literal_byte(literal)?;
                    class.push(Range {
                        start: byte,
                        end: byte,
                    });
                }
            },
            SetNode::Range { start, end, .. } => match accumulator {
                Accumulator::Unicode(class) => {
                    class.push(Range::new(u32::from(start.c), u32::from(end.c)))
                }
                Accumulator::Bytes(class) => {
                    let first = self.class_literal_byte(start)?;
                    let last = self.class_literal_byte(end)?;
                    class.push(Range::new(first, last));
                }
            },
            SetNode::Ascii {
                span,
                kind,
                negated,
            } => match accumulator {
                Accumulator::Unicode(class) => {
                    let mut ascii = ClassUnicode::new(ascii_ranges(*kind));
                    self.unicode_fold_and_negate(*negated, &mut ascii);
                    class.union(&ascii);
                }
                Accumulator::Bytes(class) => {
                    let mut ascii = ClassBytes::new(ascii_ranges(*kind));
                    self.bytes_fold_and_negate(*span, *negated, &mut ascii)?;
                    class.union(&ascii);
                }
            },
            SetNode::Unicode(unicode) => {
                let found = self.unicode_class(unicode)?;
                match accumulator {
                    Accumulator::Unicode(class) => class.union(&found),
                    Accumulator::Bytes(_) => {
                        unreachable!("Unicode classes are rejected without Unicode")
                    }
                }
            }
            SetNode::Perl(perl) => match accumulator {
                Accumulator::Unicode(class) => class.union(&self.perl_unicode(perl)),
                Accumulator::Bytes(class) => class.union(&self.perl_bytes(perl)?),
            },
            SetNode::Bracketed { span, negated, set } => match accumulator {
                Accumulator::Unicode(class) => {
                    let mut inner = Accumulator::Unicode(ClassUnicode::empty());
                    self.set(*set, &mut inner)?;
                    let Accumulator::Unicode(mut inner) = inner else {
                        unreachable!("unicode accumulator")
                    };
                    self.unicode_fold_and_negate(*negated, &mut inner);
                    class.union(&inner);
                }
                Accumulator::Bytes(class) => {
                    let mut inner = Accumulator::Bytes(ClassBytes::empty());
                    self.set(*set, &mut inner)?;
                    let Accumulator::Bytes(mut inner) = inner else {
                        unreachable!("byte accumulator")
                    };
                    self.bytes_fold_and_negate(*span, *negated, &mut inner)?;
                    class.union(&inner);
                }
            },
            SetNode::Union { items, .. } => {
                for item in items {
                    self.set(*item, accumulator)?;
                }
            }
            SetNode::BinaryOp { kind, lhs, rhs, .. } => {
                let fresh = || match accumulator {
                    Accumulator::Unicode(_) => Accumulator::Unicode(ClassUnicode::empty()),
                    Accumulator::Bytes(_) => Accumulator::Bytes(ClassBytes::empty()),
                };
                let mut left = fresh();
                self.set(*lhs, &mut left)?;
                let mut right = fresh();
                self.set(*rhs, &mut right)?;
                let fold = self.flags.case_insensitive();
                match (accumulator, left, right) {
                    (
                        Accumulator::Unicode(class),
                        Accumulator::Unicode(mut left),
                        Accumulator::Unicode(mut right),
                    ) => {
                        if fold {
                            right.case_fold_simple();
                            left.case_fold_simple();
                        }
                        match kind {
                            BinaryOpKind::Intersection => left.intersect(&right),
                            BinaryOpKind::Difference => left.difference(&right),
                            BinaryOpKind::SymmetricDifference => left.symmetric_difference(&right),
                        }
                        class.union(&left);
                    }
                    (
                        Accumulator::Bytes(class),
                        Accumulator::Bytes(mut left),
                        Accumulator::Bytes(mut right),
                    ) => {
                        if fold {
                            right.case_fold_simple();
                            left.case_fold_simple();
                        }
                        match kind {
                            BinaryOpKind::Intersection => left.intersect(&right),
                            BinaryOpKind::Difference => left.difference(&right),
                            BinaryOpKind::SymmetricDifference => left.symmetric_difference(&right),
                        }
                        class.union(&left);
                    }
                    _ => unreachable!("one class kind per bracketed class"),
                }
            }
        }
        Ok(())
    }
}

pub fn ascii_ranges(kind: AsciiKind) -> impl Iterator<Item = Range> {
    let table: &'static [(u8, u8)] = match kind {
        AsciiKind::Alnum => &[(b'0', b'9'), (b'A', b'Z'), (b'a', b'z')],
        AsciiKind::Alpha => &[(b'A', b'Z'), (b'a', b'z')],
        AsciiKind::Ascii => &[(0x00, 0x7F)],
        AsciiKind::Blank => &[(b'\t', b'\t'), (b' ', b' ')],
        AsciiKind::Cntrl => &[(0x00, 0x1F), (0x7F, 0x7F)],
        AsciiKind::Digit => &[(b'0', b'9')],
        AsciiKind::Graph => &[(b'!', b'~')],
        AsciiKind::Lower => &[(b'a', b'z')],
        AsciiKind::Print => &[(b' ', b'~')],
        AsciiKind::Punct => &[(b'!', b'/'), (b':', b'@'), (b'[', b'\x60'), (b'{', b'~')],
        AsciiKind::Space => &[
            (b'\t', b'\t'),
            (b'\n', b'\n'),
            (0x0B, 0x0B),
            (0x0C, 0x0C),
            (b'\r', b'\r'),
            (b' ', b' '),
        ],
        AsciiKind::Upper => &[(b'A', b'Z')],
        AsciiKind::Word => &[(b'0', b'9'), (b'A', b'Z'), (b'_', b'_'), (b'a', b'z')],
        AsciiKind::Xdigit => &[(b'0', b'9'), (b'A', b'F'), (b'a', b'f')],
    };
    table.iter().map(|&(start, end)| Range {
        start: u32::from(start),
        end: u32::from(end),
    })
}
