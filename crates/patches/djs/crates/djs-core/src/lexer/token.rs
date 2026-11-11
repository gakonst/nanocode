//! Token kinds, keyword ids and the token record.

/// What a token is. Reserved words have their own kinds; contextual keywords
/// (`let`, `async`, `of`, the TypeScript ones, ...) are [`TokenKind::Ident`] with a
/// [`Kw`] id in [`Token::kw`].
///
/// `>` is always a single [`TokenKind::Gt`]; the parser calls
/// [`Lexer::rescan_greater`](super::Lexer::rescan_greater) where a longer operator is
/// allowed. `/` and `/=` become regex literals through
/// [`Lexer::rescan_as_regex`](super::Lexer::rescan_as_regex), and `}` becomes a template
/// continuation through
/// [`Lexer::rescan_template_continuation`](super::Lexer::rescan_template_continuation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TokenKind {
    Eof,

    // Names and literals.
    /// An identifier name that is not an unescaped reserved word. Escaped reserved words
    /// are `Ident` too, with the [`Token::HAS_ESCAPE`] flag and a reserved [`Kw`].
    Ident,
    /// `#name`. The span includes the `#`.
    PrivateName,
    Number,
    BigInt,
    String,
    /// `` `...` `` (a template without substitutions).
    NoSubstTemplate,
    /// `` `...${ ``
    TemplateHead,
    /// `}...${`
    TemplateMiddle,
    /// `` }...` ``
    TemplateTail,
    /// `/body/flags`
    Regex,

    // Punctuators.
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    Ellipsis,
    Semicolon,
    Comma,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    StarStar,
    PlusPlus,
    MinusMinus,
    Shl,
    Shr,
    UShr,
    Amp,
    Pipe,
    Caret,
    Bang,
    Tilde,
    AmpAmp,
    PipePipe,
    QuestionQuestion,
    Question,
    QuestionDot,
    Colon,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    StarStarAssign,
    ShlAssign,
    ShrAssign,
    UShrAssign,
    AmpAssign,
    PipeAssign,
    CaretAssign,
    AmpAmpAssign,
    PipePipeAssign,
    QuestionQuestionAssign,
    Arrow,
    At,

    // Reserved words (always reserved; the engine is always strict, but the strict-only
    // reserved words such as `let` and `yield` are contextual, see `Kw`).
    Break,
    Case,
    Catch,
    Class,
    Const,
    Continue,
    Debugger,
    Default,
    Delete,
    Do,
    Else,
    Enum,
    Export,
    Extends,
    False,
    Finally,
    For,
    Function,
    If,
    Import,
    In,
    Instanceof,
    New,
    Null,
    Return,
    Super,
    Switch,
    This,
    Throw,
    True,
    Try,
    Typeof,
    Var,
    Void,
    While,
    With,
}

impl TokenKind {
    /// Reserved words, which are valid as an `IdentifierName` (property names, ...).
    pub fn is_reserved_word(self) -> bool {
        (self as u8) >= (TokenKind::Break as u8)
    }

    /// `Ident` or a reserved word: anything valid as an `IdentifierName`.
    pub fn is_identifier_name(self) -> bool {
        self == TokenKind::Ident || self.is_reserved_word()
    }

    /// Assignment operators, `=` included.
    pub fn is_assign_op(self) -> bool {
        use TokenKind::*;
        matches!(
            self,
            Assign
                | PlusAssign
                | MinusAssign
                | StarAssign
                | SlashAssign
                | PercentAssign
                | StarStarAssign
                | ShlAssign
                | ShrAssign
                | UShrAssign
                | AmpAssign
                | PipeAssign
                | CaretAssign
                | AmpAmpAssign
                | PipePipeAssign
                | QuestionQuestionAssign
        )
    }
}

/// Keyword id of an identifier name: every reserved word plus the contextual keywords
/// the parser (and later the TypeScript type skipper) needs to recognize quickly.
///
/// A token's `kw` is set for [`TokenKind::Ident`], for reserved word kinds and for
/// [`TokenKind::PrivateName`] (for the part after `#`). It is computed from the cooked
/// name, so `l\u0065t` has `Kw::Let`; check [`Token::HAS_ESCAPE`] before treating a
/// token as a keyword.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kw {
    None,
    // Reserved words, in the same order as in `TokenKind`.
    Break,
    Case,
    Catch,
    Class,
    Const,
    Continue,
    Debugger,
    Default,
    Delete,
    Do,
    Else,
    Enum,
    Export,
    Extends,
    False,
    Finally,
    For,
    Function,
    If,
    Import,
    In,
    Instanceof,
    New,
    Null,
    Return,
    Super,
    Switch,
    This,
    Throw,
    True,
    Try,
    Typeof,
    Var,
    Void,
    While,
    With,
    // Contextual keywords (identifiers in most positions).
    // `let static yield implements interface package private protected public` are
    // reserved in strict code: see `Kw::is_strict_reserved`.
    Let,
    Static,
    Yield,
    Await,
    Async,
    Of,
    Get,
    Set,
    Target,
    Meta,
    As,
    From,
    Package,
    Eval,
    Arguments,
    Constructor,
    Using,
    Accessor,
    Type,
    Interface,
    Declare,
    Abstract,
    Readonly,
    Namespace,
    Module,
    Keyof,
    Infer,
    Is,
    Asserts,
    Satisfies,
    Override,
    Implements,
    Private,
    Protected,
    Public,
    Unique,
    Global,
    Out,
}

impl Kw {
    /// Looks up the keyword id of a cooked identifier name: a perfect hash of the first
    /// two bytes, the last byte and the length into a 256-entry table, then one compare
    /// (0092: the decision tree this replaces was 4.5 KB of wasm).
    pub fn lookup(s: &[u8]) -> Kw {
        if s.len() < 2 || s.len() > KW_MAX_LEN {
            return Kw::None;
        }
        let i = KW_TABLE[kw_hash(s)] as usize;
        if KW_NAMES[i].as_bytes() == s {
            KW_ALL[i]
        } else {
            Kw::None
        }
    }

    pub fn as_str(self) -> &'static str {
        KW_NAMES[self as usize]
    }

    /// The token kind of a reserved word, `None` for contextual keywords.
    pub fn reserved_kind(self) -> Option<TokenKind> {
        Some(match self {
            Kw::Break => TokenKind::Break,
            Kw::Case => TokenKind::Case,
            Kw::Catch => TokenKind::Catch,
            Kw::Class => TokenKind::Class,
            Kw::Const => TokenKind::Const,
            Kw::Continue => TokenKind::Continue,
            Kw::Debugger => TokenKind::Debugger,
            Kw::Default => TokenKind::Default,
            Kw::Delete => TokenKind::Delete,
            Kw::Do => TokenKind::Do,
            Kw::Else => TokenKind::Else,
            Kw::Enum => TokenKind::Enum,
            Kw::Export => TokenKind::Export,
            Kw::Extends => TokenKind::Extends,
            Kw::False => TokenKind::False,
            Kw::Finally => TokenKind::Finally,
            Kw::For => TokenKind::For,
            Kw::Function => TokenKind::Function,
            Kw::If => TokenKind::If,
            Kw::Import => TokenKind::Import,
            Kw::In => TokenKind::In,
            Kw::Instanceof => TokenKind::Instanceof,
            Kw::New => TokenKind::New,
            Kw::Null => TokenKind::Null,
            Kw::Return => TokenKind::Return,
            Kw::Super => TokenKind::Super,
            Kw::Switch => TokenKind::Switch,
            Kw::This => TokenKind::This,
            Kw::Throw => TokenKind::Throw,
            Kw::True => TokenKind::True,
            Kw::Try => TokenKind::Try,
            Kw::Typeof => TokenKind::Typeof,
            Kw::Var => TokenKind::Var,
            Kw::Void => TokenKind::Void,
            Kw::While => TokenKind::While,
            Kw::With => TokenKind::With,
            _ => return None,
        })
    }

    /// Reserved in strict mode code (ECMA-262 12.7.2), in addition to the reserved words.
    pub fn is_strict_reserved(self) -> bool {
        matches!(
            self,
            Kw::Let
                | Kw::Static
                | Kw::Yield
                | Kw::Implements
                | Kw::Interface
                | Kw::Package
                | Kw::Private
                | Kw::Protected
                | Kw::Public
        )
    }
}

/// A token: kind, keyword id, flags and the byte span `start..end` in the source.
///
/// Values (names, cooked strings, numbers) are not stored; the parser asks the lexer
/// for them from the span (see `Lexer::ident_value`, `Lexer::cook_string`, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub kw: Kw,
    pub flags: u8,
    pub start: u32,
    pub end: u32,
}

impl Token {
    /// A line terminator (or a multi-line comment containing one) precedes the token.
    pub const NEWLINE_BEFORE: u8 = 1 << 0;
    /// The value differs from the source text: an identifier or private name with a
    /// `\u` escape, a string with a `\`, a template part with a `\` or a CR.
    pub const HAS_ESCAPE: u8 = 1 << 1;
    /// A template part contains a CR, so its raw value needs CR/CRLF normalization.
    pub const HAS_CR: u8 = 1 << 2;
    /// A template part contains an invalid escape: its cooked value is `undefined`
    /// (valid only in tagged templates).
    pub const INVALID_ESCAPE: u8 = 1 << 3;
    /// A numeric literal contains `_` separators.
    pub const HAS_SEPARATOR: u8 = 1 << 4;

    pub fn newline_before(&self) -> bool {
        self.flags & Self::NEWLINE_BEFORE != 0
    }

    pub fn has_escape(&self) -> bool {
        self.flags & Self::HAS_ESCAPE != 0
    }

    pub fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    pub fn span(&self) -> Span {
        Span {
            start: self.start,
            end: self.end,
        }
    }

    /// The unescaped contextual keyword `kw` (`async`, `of`, `get`, ...).
    pub fn is_contextual(&self, kw: Kw) -> bool {
        self.kind == TokenKind::Ident && self.kw == kw && !self.has_escape()
    }

    /// An identifier whose cooked name is a reserved word (`\u0069f`). Valid as an
    /// `IdentifierName`, but never as an identifier reference or binding.
    pub fn is_escaped_reserved(&self) -> bool {
        self.kind == TokenKind::Ident && self.kw.reserved_kind().is_some()
    }
}

/// A byte range `start..end` in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The bytes of this span. Out-of-range spans give an empty slice.
    pub fn slice<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        src.get(self.start as usize..self.end as usize)
            .unwrap_or(&[])
    }
}

/// Every [`Kw`], by id.
const KW_ALL: [Kw; 75] = [
    Kw::None,
    Kw::Break,
    Kw::Case,
    Kw::Catch,
    Kw::Class,
    Kw::Const,
    Kw::Continue,
    Kw::Debugger,
    Kw::Default,
    Kw::Delete,
    Kw::Do,
    Kw::Else,
    Kw::Enum,
    Kw::Export,
    Kw::Extends,
    Kw::False,
    Kw::Finally,
    Kw::For,
    Kw::Function,
    Kw::If,
    Kw::Import,
    Kw::In,
    Kw::Instanceof,
    Kw::New,
    Kw::Null,
    Kw::Return,
    Kw::Super,
    Kw::Switch,
    Kw::This,
    Kw::Throw,
    Kw::True,
    Kw::Try,
    Kw::Typeof,
    Kw::Var,
    Kw::Void,
    Kw::While,
    Kw::With,
    Kw::Let,
    Kw::Static,
    Kw::Yield,
    Kw::Await,
    Kw::Async,
    Kw::Of,
    Kw::Get,
    Kw::Set,
    Kw::Target,
    Kw::Meta,
    Kw::As,
    Kw::From,
    Kw::Package,
    Kw::Eval,
    Kw::Arguments,
    Kw::Constructor,
    Kw::Using,
    Kw::Accessor,
    Kw::Type,
    Kw::Interface,
    Kw::Declare,
    Kw::Abstract,
    Kw::Readonly,
    Kw::Namespace,
    Kw::Module,
    Kw::Keyof,
    Kw::Infer,
    Kw::Is,
    Kw::Asserts,
    Kw::Satisfies,
    Kw::Override,
    Kw::Implements,
    Kw::Private,
    Kw::Protected,
    Kw::Public,
    Kw::Unique,
    Kw::Global,
    Kw::Out,
];

/// The text of every [`Kw`], by id.
const KW_NAMES: [&str; 75] = [
    "",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "let",
    "static",
    "yield",
    "await",
    "async",
    "of",
    "get",
    "set",
    "target",
    "meta",
    "as",
    "from",
    "package",
    "eval",
    "arguments",
    "constructor",
    "using",
    "accessor",
    "type",
    "interface",
    "declare",
    "abstract",
    "readonly",
    "namespace",
    "module",
    "keyof",
    "infer",
    "is",
    "asserts",
    "satisfies",
    "override",
    "implements",
    "private",
    "protected",
    "public",
    "unique",
    "global",
    "out",
];

const KW_MAX_LEN: usize = 11;

/// The perfect hash of [`Kw::lookup`] (the multiplier was searched for: no two keywords
/// collide, which `KW_TABLE`'s construction checks).
const fn kw_hash(s: &[u8]) -> usize {
    let x =
        s[0] as u32 | (s[1] as u32) << 8 | (s[s.len() - 1] as u32) << 16 | (s.len() as u32) << 24;
    (x.wrapping_mul(0x7849_be11) >> 24) as usize
}

/// Hash → keyword id (0: none).
const KW_TABLE: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 1;
    while i < KW_ALL.len() {
        assert!(KW_ALL[i] as usize == i);
        let s = KW_NAMES[i].as_bytes();
        assert!(s.len() >= 2 && s.len() <= KW_MAX_LEN);
        let h = kw_hash(s);
        assert!(t[h] == 0, "keyword hash collision");
        t[h] = i as u8;
        i += 1;
    }
    t
};
