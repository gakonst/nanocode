use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;

const MAX_ENTITY_EXPANSION_LENGTH_CHARS: usize = 1_000_000;
const MAX_ENTITY_EXPANSION_DEPTH: u8 = 10;
const MAX_NAME_LENGTH_BYTES: usize = 1 << 18;
const MAX_ATTRIBUTES: usize = 1 << 16;
const MAX_ATTRIBUTE_LENGTH_BYTES: usize = 1 << 30;
const MAX_DATA_LENGTH_BYTES: usize = 1 << 30;
const POSITION_QUEUE_CAPACITY: usize = 16;
const ATTRIBUTES_HASHED_DUPLICATE_SCAN_FROM: usize = 8;
const ENCODED_CHARACTER_MAX_BYTES: usize = 4;
const UTF8_BYTE_ORDER_MARK: [u8; 3] = [0xEF, 0xBB, 0xBF];
const UTF16_BIG_ENDIAN_BYTE_ORDER_MARK: [u8; 2] = [0xFE, 0xFF];
const UTF16_LITTLE_ENDIAN_BYTE_ORDER_MARK: [u8; 2] = [0xFF, 0xFE];
const UTF16_UNIT_BYTES: usize = 2;
const DOCTYPE_KEYWORD: &[u8] = b"DOCTYPE";
const CDATA_KEYWORD: &[u8] = b"CDATA[";
const DOCTYPE_MARKUP: &str = "<!DOCTYPE";
const MARKUP_DECLARATION_MARKUP: &str = "<!";
const XML_PREFIX: &str = "xml";
const XML_NAMESPACE_URI: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_PREFIX: &str = "xmlns";
const XMLNS_NAMESPACE_URI: &str = "http://www.w3.org/2000/xmlns/";
const NO_PREFIX: &str = "";
const SYSTEM_KEYWORD: &str = "SYSTEM";
const PUBLIC_KEYWORD: &str = "PUBLIC";
const DECIMAL_RADIX: u32 = 10;
const HEXADECIMAL_RADIX: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextPosition {
    row: u64,
    column: u64,
}

impl TextPosition {
    const START: Self = Self { row: 0, column: 0 };

    pub(crate) fn row(&self) -> u64 {
        self.row
    }

    pub(crate) fn column(&self) -> u64 {
        self.column
    }

    fn advance_column(&mut self) {
        self.column += 1;
    }

    fn new_line(&mut self) {
        self.row += 1;
        self.column = 0;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QualifiedName {
    pub(crate) local_name: String,
    pub(crate) namespace: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attribute {
    pub(crate) name: QualifiedName,
    pub(crate) value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum XmlEvent {
    StartDocument,
    Doctype,
    StartElement {
        name: QualifiedName,
        attributes: Vec<Attribute>,
    },
    EndElement {
        name: QualifiedName,
    },
    ProcessingInstruction {
        name: String,
        data: Option<String>,
    },
    CData(String),
    Comment(String),
    Characters(String),
    EndDocument,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReaderError {
    Io,
    Decode,
    TruncatedCharacter,
    UnclosedCData,
    UnterminatedMarkup,
    UnexpectedMarkup,
    EntityTooBig,
    UnexpectedTokenOutsideRoot,
    InvalidCharacter,
    LimitExceeded,
    UnexpectedToken,
    InvalidQualifiedName,
    InvalidNamePrefix,
    UnboundPrefix,
    RedefinedAttribute,
    MismatchedClosingTag,
    EmptyEntity,
    UndefinedEntity,
    InvalidCharacterReference,
    UnexpectedOpeningTag,
    InvalidProcessingInstruction,
    UnsupportedEncoding,
    ConflictingEncoding,
    InvalidVersion,
    InvalidStandalone,
    InvalidNamespaceDeclaration,
    UnknownMarkupDeclaration,
    NoRootElement,
    UnexpectedEndOfDocument,
    UnbalancedRootElement,
}

fn is_whitespace(character: char) -> bool {
    matches!(character, '\u{20}' | '\u{0A}' | '\u{09}' | '\u{0D}')
}

fn is_pubid_character(character: char) -> bool {
    matches!(character, '\u{20}' | '\u{0D}' | '\u{0A}' | 'a'..='z' | 'A'..='Z' | '0'..='9'
        | '-' | '\'' | '(' | ')' | '+' | ',' | '.' | '/' | ':' | '=' | '?' | ';'
        | '!' | '*' | '#' | '@' | '$' | '_' | '%')
}

fn is_xml10_character(character: char) -> bool {
    matches!(character, '\u{09}' | '\u{0A}' | '\u{0D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

fn is_xml11_character(character: char) -> bool {
    matches!(character, '\u{01}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

fn is_xml11_unrestricted_character(character: char) -> bool {
    is_xml11_character(character)
        && !matches!(character, '\u{01}'..='\u{08}' | '\u{0B}'..='\u{0C}' | '\u{0E}'..='\u{1F}' | '\u{7F}'..='\u{84}' | '\u{86}'..='\u{9F}')
}

fn is_name_start_character(character: char) -> bool {
    matches!(character,
        ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

fn is_name_character(character: char) -> bool {
    is_name_start_character(character)
        || matches!(character, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

fn extends_name(name: &str, character: char) -> bool {
    if name.is_empty() {
        is_name_start_character(character)
    } else {
        is_name_character(character)
    }
}

fn label_matches(label: &str, declared: &str) -> bool {
    label
        .bytes()
        .zip(declared.bytes())
        .all(|(expected, actual)| expected == actual.to_ascii_lowercase())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Unknown,
    Default,
    Utf8,
    Latin1,
    Ascii,
    Utf16BigEndian,
    Utf16LittleEndian,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeclaredEncoding {
    Utf8,
    Latin1,
    Utf16,
    Ascii,
}

impl DeclaredEncoding {
    fn parse(declared: &str) -> Option<Self> {
        let matches_any =
            |labels: [&str; 2]| labels.iter().any(|label| label_matches(label, declared));
        if matches_any(["utf-8", "utf8"]) {
            Some(Self::Utf8)
        } else if matches_any(["iso-8859-1", "latin1"]) {
            Some(Self::Latin1)
        } else if matches_any(["utf-16", "utf16"]) {
            Some(Self::Utf16)
        } else if matches_any(["ascii", "us-ascii"]) {
            Some(Self::Ascii)
        } else {
            None
        }
    }
}

struct Decoder {
    encoding: Encoding,
}

impl Decoder {
    fn next_character<R: Read>(&mut self, source: &mut R) -> Result<Option<char>, ReaderError> {
        let mut sequence = [0u8; ENCODED_CHARACTER_MAX_BYTES];
        let mut length = 0usize;
        while length < ENCODED_CHARACTER_MAX_BYTES {
            let byte = match source.by_ref().bytes().next() {
                Some(Ok(byte)) => byte,
                Some(Err(_)) => return Err(ReaderError::Io),
                None if length == 0 => return Ok(None),
                None => return Err(ReaderError::TruncatedCharacter),
            };
            match self.encoding {
                Encoding::Utf8 | Encoding::Default => {
                    if length == 0 && byte.is_ascii() {
                        return Ok(Some(char::from(byte)));
                    }
                    sequence[length] = byte;
                    length += 1;
                    match core::str::from_utf8(&sequence[..length]) {
                        Ok(text) => return Ok(text.chars().next()),
                        Err(_) if length < ENCODED_CHARACTER_MAX_BYTES => {}
                        Err(_) => return Err(ReaderError::Decode),
                    }
                }
                Encoding::Latin1 => return Ok(Some(char::from(byte))),
                Encoding::Ascii => {
                    return if byte.is_ascii() {
                        Ok(Some(char::from(byte)))
                    } else {
                        Err(ReaderError::Decode)
                    };
                }
                Encoding::Unknown => {
                    sequence[length] = byte;
                    length += 1;
                    if let Some(character) = self.sniff(&sequence[..length], &mut length) {
                        return Ok(Some(character));
                    }
                }
                Encoding::Utf16BigEndian | Encoding::Utf16LittleEndian => {
                    sequence[length] = byte;
                    length += 1;
                    if length == UTF16_UNIT_BYTES {
                        let unit = self.unit(sequence[0], sequence[1]);
                        if let Some(Ok(character)) = char::decode_utf16([unit]).next() {
                            return Ok(Some(character));
                        }
                    } else if length == ENCODED_CHARACTER_MAX_BYTES {
                        let units = [
                            self.unit(sequence[0], sequence[1]),
                            self.unit(sequence[2], sequence[3]),
                        ];
                        return match char::decode_utf16(units).next() {
                            Some(Ok(character)) => Ok(Some(character)),
                            Some(Err(_)) | None => Err(ReaderError::Decode),
                        };
                    }
                }
            }
        }
        Err(ReaderError::Decode)
    }

    fn unit(&self, first: u8, second: u8) -> u16 {
        if self.encoding == Encoding::Utf16BigEndian {
            u16::from_be_bytes([first, second])
        } else {
            u16::from_le_bytes([first, second])
        }
    }

    fn sniff(&mut self, prefix: &[u8], length: &mut usize) -> Option<char> {
        if UTF8_BYTE_ORDER_MARK.starts_with(prefix) {
            if prefix.len() == UTF8_BYTE_ORDER_MARK.len() {
                *length = 0;
                self.encoding = Encoding::Utf8;
            }
        } else if UTF16_BIG_ENDIAN_BYTE_ORDER_MARK.starts_with(prefix) {
            if prefix.len() == UTF16_BIG_ENDIAN_BYTE_ORDER_MARK.len() {
                *length = 0;
                self.encoding = Encoding::Utf16BigEndian;
            }
        } else if UTF16_LITTLE_ENDIAN_BYTE_ORDER_MARK.starts_with(prefix) {
            if prefix.len() == UTF16_LITTLE_ENDIAN_BYTE_ORDER_MARK.len() {
                *length = 0;
                self.encoding = Encoding::Utf16LittleEndian;
            }
        } else {
            self.encoding = Encoding::Default;
            if let [byte] = prefix
                && byte.is_ascii()
            {
                return Some(char::from(*byte));
            }
        }
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Token {
    ProcessingInstructionStart,
    ProcessingInstructionEnd,
    DoctypeStart,
    OpeningTagStart,
    ClosingTagStart,
    TagEnd,
    EmptyTagEnd,
    CommentStart,
    CommentEnd,
    Character(char),
    EqualsSign,
    SingleQuote,
    DoubleQuote,
    CDataStart,
    CDataEnd,
    ReferenceStart,
    ReferenceEnd,
    MarkupDeclarationStart,
    EndOfInput,
}

impl Token {
    fn markup(self) -> Option<&'static str> {
        match self {
            Self::OpeningTagStart => Some("<"),
            Self::ProcessingInstructionStart => Some("<?"),
            Self::DoctypeStart => Some(DOCTYPE_MARKUP),
            Self::ClosingTagStart => Some("</"),
            Self::CommentStart => Some("<!--"),
            Self::CDataStart => Some("<![CDATA["),
            Self::TagEnd => Some(">"),
            Self::EmptyTagEnd => Some("/>"),
            Self::ProcessingInstructionEnd => Some("?>"),
            Self::CommentEnd => Some("-->"),
            Self::CDataEnd => Some("]]>"),
            Self::ReferenceStart => Some("&"),
            Self::ReferenceEnd => Some(";"),
            Self::EqualsSign => Some("="),
            Self::SingleQuote => Some("'"),
            Self::DoubleQuote => Some("\""),
            Self::Character(_) | Self::MarkupDeclarationStart | Self::EndOfInput => None,
        }
    }

    fn push_to(self, target: &mut String) {
        match self {
            Self::Character(character) => target.push(character),
            _ => {
                if let Some(markup) = self.markup() {
                    target.push_str(markup);
                }
            }
        }
    }

    fn displayed_length(self) -> usize {
        match self {
            Self::Character(character) => character.len_utf8(),
            Self::MarkupDeclarationStart => MARKUP_DECLARATION_MARKUP.len(),
            _ => self.markup().map_or(0, str::len),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    Single,
    Double,
}

impl Quote {
    fn from_token(token: Token) -> Option<Self> {
        match token {
            Token::SingleQuote => Some(Self::Single),
            Token::DoubleQuote => Some(Self::Double),
            _ => None,
        }
    }

    fn token(self) -> Token {
        match self {
            Self::Single => Token::SingleQuote,
            Self::Double => Token::DoubleQuote,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Closing {
    First,
    Second,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LexerContext {
    Content,
    Doctype,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LexerState {
    Normal,
    TagStarted,
    MarkupStarted,
    CommentStarted,
    DoctypeStarted(usize),
    InsideMarkupDeclaration,
    InsideDoctype,
    CDataStarted(usize),
    ProcessingInstructionClosing,
    EmptyTagClosing,
    CommentClosing(Closing),
    CDataClosing(Closing),
    InvalidCDataClosing(Closing),
    InsideComment,
    InsideCData,
    InsideProcessingInstruction,
    InsideMarkupDeclarationQuotedString(Quote),
}

struct Lexer {
    state: LexerState,
    context: LexerContext,
    decoder: Decoder,
    position: TextPosition,
    head_position: TextPosition,
    pending: VecDeque<char>,
    inside_token: bool,
    end_of_input_handled: bool,
    reparse_depth: u8,
}

impl Lexer {
    fn new() -> Self {
        Self {
            state: LexerState::Normal,
            context: LexerContext::Content,
            decoder: Decoder {
                encoding: Encoding::Unknown,
            },
            position: TextPosition::START,
            head_position: TextPosition::START,
            pending: VecDeque::new(),
            inside_token: false,
            end_of_input_handled: false,
            reparse_depth: 0,
        }
    }

    fn next_token<R: Read>(&mut self, source: &mut R) -> Result<Token, ReaderError> {
        if self.end_of_input_handled {
            return Ok(Token::EndOfInput);
        }
        if !self.inside_token {
            self.position = self.head_position;
            self.inside_token = true;
        }
        while let Some(character) = self.pending.pop_front() {
            if let Some(token) = self.dispatch(character)? {
                self.inside_token = false;
                return Ok(token);
            }
        }
        self.reparse_depth = 0;
        while let Some(character) = self.decoder.next_character(source)? {
            if character == '\n' {
                self.head_position.new_line();
            } else {
                self.head_position.advance_column();
            }
            if let Some(token) = self.dispatch(character)? {
                self.inside_token = false;
                return Ok(token);
            }
        }
        self.end_of_input()
    }

    fn end_of_input(&mut self) -> Result<Token, ReaderError> {
        self.end_of_input_handled = true;
        self.position = self.head_position;
        match self.state {
            LexerState::Normal => Ok(Token::EndOfInput),
            LexerState::InsideCData | LexerState::CDataClosing(_) => {
                Err(ReaderError::UnclosedCData)
            }
            _ => Err(ReaderError::UnterminatedMarkup),
        }
    }

    fn normal_state(&self) -> LexerState {
        match self.context {
            LexerContext::Content => LexerState::Normal,
            LexerContext::Doctype => LexerState::InsideDoctype,
        }
    }

    fn reparse(&mut self, markup: &str) -> Result<(), ReaderError> {
        if markup.is_empty() {
            return Ok(());
        }
        self.reparse_depth += 1;
        if self.reparse_depth > MAX_ENTITY_EXPANSION_DEPTH
            || self.pending.len() > MAX_ENTITY_EXPANSION_LENGTH_CHARS
        {
            return Err(ReaderError::EntityTooBig);
        }
        self.end_of_input_handled = false;
        self.pending.reserve(markup.len());
        for character in markup.chars().rev() {
            self.pending.push_front(character);
        }
        Ok(())
    }

    fn move_to(&mut self, state: LexerState) -> Option<Token> {
        self.state = state;
        None
    }

    fn emit(&mut self, state: LexerState, token: Token) -> Option<Token> {
        self.state = state;
        Some(token)
    }

    fn emit_unread(&mut self, state: LexerState, unread: &[char], token: Token) -> Option<Token> {
        for character in unread.iter().rev() {
            self.pending.push_front(*character);
        }
        self.emit(state, token)
    }

    fn emit_normal(&mut self, token: Token) -> Option<Token> {
        let state = self.normal_state();
        self.emit(state, token)
    }

    fn emit_in_context(&mut self, context: LexerContext, token: Token) -> Option<Token> {
        self.context = context;
        self.emit_normal(token)
    }

    fn match_keyword(keyword: &[u8], matched: usize, character: char) -> Result<bool, ReaderError> {
        if keyword.get(matched).copied().map(char::from) != Some(character) {
            return Err(ReaderError::UnexpectedMarkup);
        }
        Ok(matched + 1 == keyword.len())
    }

    fn dispatch(&mut self, character: char) -> Result<Option<Token>, ReaderError> {
        let token = match self.state {
            LexerState::Normal => match character {
                '<' => self.move_to(LexerState::TagStarted),
                '>' => Some(Token::TagEnd),
                '/' => self.move_to(LexerState::EmptyTagClosing),
                '=' => Some(Token::EqualsSign),
                '"' => Some(Token::DoubleQuote),
                '\'' => Some(Token::SingleQuote),
                ']' => self.move_to(LexerState::InvalidCDataClosing(Closing::First)),
                '&' => Some(Token::ReferenceStart),
                ';' => Some(Token::ReferenceEnd),
                _ => Some(Token::Character(character)),
            },
            LexerState::TagStarted => match character {
                '?' => self.emit(
                    LexerState::InsideProcessingInstruction,
                    Token::ProcessingInstructionStart,
                ),
                '/' => self.emit_normal(Token::ClosingTagStart),
                '!' => self.move_to(LexerState::MarkupStarted),
                _ if is_whitespace(character) || is_name_character(character) => {
                    let state = self.normal_state();
                    self.emit_unread(state, &[character], Token::OpeningTagStart)
                }
                _ => return Err(ReaderError::UnexpectedMarkup),
            },
            LexerState::EmptyTagClosing => match character {
                '>' => self.emit_normal(Token::EmptyTagEnd),
                _ => {
                    let state = self.normal_state();
                    self.emit_unread(state, &[character], Token::Character('/'))
                }
            },
            LexerState::MarkupStarted => match character {
                '-' => self.move_to(LexerState::CommentStarted),
                '[' => self.move_to(LexerState::CDataStarted(0)),
                'D' => self.move_to(LexerState::DoctypeStarted(1)),
                'E' | 'A' | 'N' if self.context == LexerContext::Doctype => self.emit_unread(
                    LexerState::InsideMarkupDeclaration,
                    &[character],
                    Token::MarkupDeclarationStart,
                ),
                _ => return Err(ReaderError::UnexpectedMarkup),
            },
            LexerState::CommentStarted => match character {
                '-' => self.emit(LexerState::InsideComment, Token::CommentStart),
                _ => return Err(ReaderError::UnexpectedMarkup),
            },
            LexerState::CDataStarted(matched) => {
                if Self::match_keyword(CDATA_KEYWORD, matched, character)? {
                    self.emit(LexerState::InsideCData, Token::CDataStart)
                } else {
                    self.move_to(LexerState::CDataStarted(matched + 1))
                }
            }
            LexerState::DoctypeStarted(matched) => {
                if Self::match_keyword(DOCTYPE_KEYWORD, matched, character)? {
                    self.emit_in_context(LexerContext::Doctype, Token::DoctypeStart)
                } else {
                    self.move_to(LexerState::DoctypeStarted(matched + 1))
                }
            }
            LexerState::InsideCData => match character {
                ']' => self.move_to(LexerState::CDataClosing(Closing::First)),
                _ => Some(Token::Character(character)),
            },
            LexerState::InsideComment => match character {
                '-' => self.move_to(LexerState::CommentClosing(Closing::First)),
                _ => Some(Token::Character(character)),
            },
            LexerState::InsideProcessingInstruction => match character {
                '?' => self.move_to(LexerState::ProcessingInstructionClosing),
                '<' => Some(Token::OpeningTagStart),
                '>' => Some(Token::TagEnd),
                '=' => Some(Token::EqualsSign),
                '"' => Some(Token::DoubleQuote),
                '\'' => Some(Token::SingleQuote),
                '&' => Some(Token::ReferenceStart),
                ';' => Some(Token::ReferenceEnd),
                _ => Some(Token::Character(character)),
            },
            LexerState::ProcessingInstructionClosing => match character {
                '>' => self.emit_normal(Token::ProcessingInstructionEnd),
                _ => self.emit_unread(
                    LexerState::InsideProcessingInstruction,
                    &[character],
                    Token::Character('?'),
                ),
            },
            LexerState::CommentClosing(Closing::First) => match character {
                '-' => self.move_to(LexerState::CommentClosing(Closing::Second)),
                _ => self.emit_unread(
                    LexerState::InsideComment,
                    &[character],
                    Token::Character('-'),
                ),
            },
            LexerState::CommentClosing(Closing::Second) => match character {
                '>' => self.emit_normal(Token::CommentEnd),
                _ => return Err(ReaderError::UnexpectedMarkup),
            },
            LexerState::CDataClosing(Closing::First) => match character {
                ']' => self.move_to(LexerState::CDataClosing(Closing::Second)),
                _ => self.emit_unread(LexerState::InsideCData, &[character], Token::Character(']')),
            },
            LexerState::CDataClosing(Closing::Second) => match character {
                '>' => self.emit(LexerState::Normal, Token::CDataEnd),
                _ => self.emit_unread(
                    LexerState::InsideCData,
                    &[']', character],
                    Token::Character(']'),
                ),
            },
            LexerState::InvalidCDataClosing(Closing::First) => match character {
                ']' => self.move_to(LexerState::InvalidCDataClosing(Closing::Second)),
                _ => self.emit_unread(LexerState::Normal, &[character], Token::Character(']')),
            },
            LexerState::InvalidCDataClosing(Closing::Second) => match character {
                '>' => self.emit_normal(Token::CDataEnd),
                _ => self.emit_unread(LexerState::Normal, &[']', character], Token::Character(']')),
            },
            LexerState::InsideDoctype => match character {
                '>' => self.emit_in_context(LexerContext::Content, Token::TagEnd),
                '<' => self.move_to(LexerState::TagStarted),
                '&' => Some(Token::ReferenceStart),
                ';' => Some(Token::ReferenceEnd),
                '"' => Some(Token::DoubleQuote),
                '\'' => Some(Token::SingleQuote),
                _ => Some(Token::Character(character)),
            },
            LexerState::InsideMarkupDeclaration => match character {
                '<' => return Err(ReaderError::UnexpectedMarkup),
                '>' => self.emit_normal(Token::TagEnd),
                '&' => Some(Token::ReferenceStart),
                ';' => Some(Token::ReferenceEnd),
                '"' => self.emit(
                    LexerState::InsideMarkupDeclarationQuotedString(Quote::Double),
                    Token::DoubleQuote,
                ),
                '\'' => self.emit(
                    LexerState::InsideMarkupDeclarationQuotedString(Quote::Single),
                    Token::SingleQuote,
                ),
                _ => Some(Token::Character(character)),
            },
            LexerState::InsideMarkupDeclarationQuotedString(quote) => match (character, quote) {
                ('"', Quote::Double) => {
                    self.emit(LexerState::InsideMarkupDeclaration, Token::DoubleQuote)
                }
                ('\'', Quote::Single) => {
                    self.emit(LexerState::InsideMarkupDeclaration, Token::SingleQuote)
                }
                _ => Some(Token::Character(character)),
            },
        };
        Ok(token)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Encountered {
    Nothing,
    AnyCharacters,
    Declaration,
    Comment,
    Doctype,
    Element,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum XmlVersion {
    Xml10,
    Xml11,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpeningTag {
    InsideName,
    InsideTag,
    InsideAttributeName,
    AfterAttributeName,
    InsideAttributeValue,
    AfterAttributeValue,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClosingTag {
    InsideName,
    AfterName,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcessingInstruction {
    InsideName,
    InsideData,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Declaration {
    BeforeVersion,
    InsideVersion,
    AfterVersion,
    InsideVersionValue,
    AfterVersionValue,
    BeforeEncoding,
    InsideEncoding,
    AfterEncoding,
    InsideEncodingValue,
    AfterEncodingValue,
    BeforeStandalone,
    InsideStandalone,
    AfterStandalone,
    InsideStandaloneValue,
    AfterStandaloneValue,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Doctype {
    BeforeName,
    Name,
    Outside,
    ExternalIdKeyword,
    BeforeSystemLiteral,
    SystemLiteral,
    BeforePublicId,
    PublicId,
    InternalSubset,
    String,
    InsideName,
    BeforeEntityName,
    EntityName,
    BeforeEntityValue,
    EntityValue,
    NumericReferenceStart,
    NumericReference,
    ParameterReferenceInValue,
    ParameterReferenceInSubset,
    ParameterDefinitionStart,
    ParameterDefinition,
    IgnoredProcessingInstruction,
    SkippedDeclaration,
    Comment,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParserState {
    DocumentStart,
    OutsideTag,
    InsideOpeningTag(OpeningTag),
    InsideClosingTag(ClosingTag),
    InsideProcessingInstruction(ProcessingInstruction),
    InsideComment,
    InsideCData,
    InsideDeclaration(Declaration),
    InsideDoctype(Doctype),
    InsideReference,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NameTarget {
    Attribute,
    OpeningTag,
    ClosingTag,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Literal {
    System,
    Public,
}

#[derive(Clone, PartialEq, Eq)]
struct Name {
    prefix: Option<String>,
    local_name: String,
    namespace: Option<String>,
}

impl Name {
    fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split(':');
        let (prefix, local_name) = match (parts.next(), parts.next(), parts.next()) {
            (Some(prefix), Some(local_name), None)
                if !prefix.is_empty() && !local_name.is_empty() =>
            {
                (Some(prefix.to_owned()), local_name)
            }
            (Some(local_name), None, None) if !local_name.is_empty() => (None, local_name),
            _ => return None,
        };
        Some(Self {
            prefix,
            local_name: local_name.to_owned(),
            namespace: None,
        })
    }

    fn has_reserved_prefix(&self) -> bool {
        matches!(self.prefix.as_deref(), Some(XML_PREFIX | XMLNS_PREFIX))
    }

    fn qualified(self) -> QualifiedName {
        QualifiedName {
            local_name: self.local_name,
            namespace: self.namespace,
        }
    }
}

enum NameStep {
    Pending,
    Complete(Token, Name),
    Failed(ReaderError),
}

enum ValueStep {
    Pending,
    Complete(String),
    Failed(ReaderError),
}

enum Finish {
    Ended,
    Failed(ReaderError),
}

type Step = Option<Result<XmlEvent, ReaderError>>;

struct Parser {
    lexer: Lexer,
    state: ParserState,
    state_after_reference: ParserState,
    text: String,
    qualified_name: String,
    seen_prefix_separator: bool,
    entities: HashMap<String, String>,
    namespaces: Vec<Vec<(String, String)>>,
    markup_name: String,
    reference: String,
    version: Option<XmlVersion>,
    declared_encoding: Option<String>,
    element_name: Option<Name>,
    quote: Option<Quote>,
    attribute_name: Option<Name>,
    attributes: Vec<(Name, String)>,
    doctype_length: usize,
    finish: Option<Finish>,
    next_event: Option<XmlEvent>,
    open_elements: Vec<Name>,
    position: TextPosition,
    queued_positions: VecDeque<TextPosition>,
    encountered: Encountered,
    pop_namespace: bool,
}

impl Parser {
    fn new() -> Self {
        let base_namespaces = vec![
            (XML_PREFIX.to_owned(), XML_NAMESPACE_URI.to_owned()),
            (XMLNS_PREFIX.to_owned(), XMLNS_NAMESPACE_URI.to_owned()),
            (NO_PREFIX.to_owned(), String::new()),
        ];
        Self {
            lexer: Lexer::new(),
            state: ParserState::DocumentStart,
            state_after_reference: ParserState::OutsideTag,
            text: String::new(),
            qualified_name: String::new(),
            seen_prefix_separator: false,
            entities: HashMap::new(),
            namespaces: vec![base_namespaces],
            markup_name: String::new(),
            reference: String::new(),
            version: None,
            declared_encoding: None,
            element_name: None,
            quote: None,
            attribute_name: None,
            attributes: Vec::new(),
            doctype_length: 0,
            finish: None,
            next_event: None,
            open_elements: Vec::new(),
            position: TextPosition::START,
            queued_positions: VecDeque::with_capacity(POSITION_QUEUE_CAPACITY),
            encountered: Encountered::Nothing,
            pop_namespace: false,
        }
    }

    fn next<R: Read>(&mut self, source: &mut R) -> Result<XmlEvent, ReaderError> {
        match self.finish {
            Some(Finish::Ended) => return Ok(XmlEvent::EndDocument),
            Some(Finish::Failed(error)) => return Err(error),
            None => {}
        }
        if let Some(event) = self.next_event.take() {
            return Ok(event);
        }
        if self.pop_namespace {
            self.pop_namespace = false;
            assert!(self.namespaces.len() > 1);
            self.namespaces.pop();
        }
        loop {
            let token = match self.lexer.next_token(source) {
                Ok(token) => token,
                Err(error) => {
                    self.next_position();
                    return self.fail(error);
                }
            };
            if token == Token::EndOfInput {
                self.next_position();
                return self.end_of_input();
            }
            match self.dispatch(token) {
                None => {}
                Some(Ok(event)) => {
                    self.next_position();
                    return Ok(event);
                }
                Some(Err(error)) => {
                    self.next_position();
                    return self.fail(error);
                }
            }
        }
    }

    fn fail(&mut self, error: ReaderError) -> Result<XmlEvent, ReaderError> {
        self.finish = Some(Finish::Failed(error));
        Err(error)
    }

    fn end_of_input(&mut self) -> Result<XmlEvent, ReaderError> {
        let error = if !self.open_elements.is_empty() {
            ReaderError::UnbalancedRootElement
        } else if self.encountered == Encountered::Element && self.state == ParserState::OutsideTag
        {
            self.finish = Some(Finish::Ended);
            return Ok(XmlEvent::EndDocument);
        } else if self.encountered < Encountered::Element {
            ReaderError::NoRootElement
        } else {
            ReaderError::UnexpectedEndOfDocument
        };
        self.fail(error)
    }

    fn next_position(&mut self) {
        self.position = match self.queued_positions.pop_front() {
            Some(position) => position,
            None => self.lexer.position,
        };
    }

    fn push_position(&mut self) {
        assert!(self.queued_positions.len() < POSITION_QUEUE_CAPACITY);
        if self.queued_positions.len() + 1 != POSITION_QUEUE_CAPACITY {
            self.queued_positions.push_back(self.lexer.position);
        } else if let Some(position) = self.queued_positions.pop_front() {
            self.position = position;
        }
    }

    fn set_encountered(&mut self, encountered: Encountered) -> Step {
        if encountered <= self.encountered {
            return None;
        }
        let previous = self.encountered;
        self.encountered = encountered;
        if previous == Encountered::Nothing {
            self.push_position();
            Some(Ok(XmlEvent::StartDocument))
        } else {
            None
        }
    }

    fn depth(&self) -> usize {
        self.open_elements.len()
    }

    fn take_text(&mut self) -> String {
        core::mem::take(&mut self.text)
    }

    fn enter(&mut self, state: ParserState, step: Step) -> Step {
        self.state = state;
        step
    }

    fn proceed(&mut self, state: ParserState) -> Step {
        self.state = state;
        None
    }

    fn is_valid_character(&self, character: char) -> bool {
        if self.version == Some(XmlVersion::Xml11) {
            is_xml11_character(character)
        } else {
            is_xml10_character(character)
        }
    }

    fn is_valid_unrestricted_character(&self, character: char) -> bool {
        if self.version == Some(XmlVersion::Xml11) {
            is_xml11_unrestricted_character(character)
        } else {
            is_xml10_character(character)
        }
    }

    fn lookup_namespace(&self, prefix: &str) -> Option<&str> {
        self.namespaces.iter().rev().find_map(|frame| {
            frame
                .iter()
                .find(|(bound, _)| bound == prefix)
                .map(|(_, uri)| uri.as_str())
        })
    }

    fn bind_namespace(&mut self, prefix: String, uri: String) {
        if let Some(frame) = self.namespaces.last_mut()
            && !frame.iter().any(|(bound, _)| *bound == prefix)
        {
            frame.push((prefix, uri));
        }
    }

    fn resolve(&self, prefix: &str) -> Result<Option<String>, ReaderError> {
        match self.lookup_namespace(prefix) {
            Some("") => Ok(None),
            Some(uri) => Ok(Some(uri.to_owned())),
            None => Err(ReaderError::UnboundPrefix),
        }
    }

    fn dispatch(&mut self, token: Token) -> Step {
        match self.state {
            ParserState::DocumentStart => self.document_start(token),
            ParserState::OutsideTag => self.outside_tag(token),
            ParserState::InsideOpeningTag(substate) => self.inside_opening_tag(token, substate),
            ParserState::InsideClosingTag(substate) => self.inside_closing_tag(token, substate),
            ParserState::InsideProcessingInstruction(substate) => {
                self.inside_processing_instruction(token, substate)
            }
            ParserState::InsideComment => self.inside_comment(token),
            ParserState::InsideCData => self.inside_cdata(token),
            ParserState::InsideDeclaration(substate) => self.inside_declaration(token, substate),
            ParserState::InsideDoctype(substate) => self.inside_doctype(token, substate),
            ParserState::InsideReference => self.inside_reference(token),
        }
    }

    fn push_text_markup(&mut self, markup: &str) -> Step {
        if self.text.is_empty() {
            self.push_position();
        } else if self.text.len() > MAX_DATA_LENGTH_BYTES {
            return Some(Err(ReaderError::LimitExceeded));
        }
        self.text.push_str(markup);
        None
    }

    fn outside_tag(&mut self, token: Token) -> Step {
        match token {
            Token::Character(character) => {
                if is_whitespace(character) {
                    if self.depth() == 0 {
                        return None;
                    }
                } else if self.depth() == 0 {
                    return Some(Err(ReaderError::UnexpectedTokenOutsideRoot));
                }
                if !self.is_valid_unrestricted_character(character) {
                    return Some(Err(ReaderError::InvalidCharacter));
                }
                let mut encoded = [0u8; ENCODED_CHARACTER_MAX_BYTES];
                self.push_text_markup(character.encode_utf8(&mut encoded))
            }
            Token::CommentEnd
            | Token::TagEnd
            | Token::EqualsSign
            | Token::DoubleQuote
            | Token::SingleQuote
            | Token::ProcessingInstructionEnd
            | Token::EmptyTagEnd => {
                if self.depth() == 0 {
                    return Some(Err(ReaderError::UnexpectedTokenOutsideRoot));
                }
                let markup = token.markup()?;
                self.push_text_markup(markup)
            }
            Token::ReferenceStart if self.depth() > 0 => {
                self.state_after_reference = ParserState::OutsideTag;
                self.proceed(ParserState::InsideReference)
            }
            Token::ReferenceEnd if self.depth() > 0 => {
                if self.text.len() > MAX_DATA_LENGTH_BYTES {
                    return Some(Err(ReaderError::LimitExceeded));
                }
                self.text.push(';');
                None
            }
            _ => self.markup_outside_tag(token),
        }
    }

    fn markup_outside_tag(&mut self, token: Token) -> Step {
        let mut next_event = if self.text.is_empty() {
            None
        } else {
            Some(Ok(XmlEvent::Characters(self.take_text())))
        };
        self.push_position();
        match token {
            Token::OpeningTagStart => {
                if let Some(event) = self.set_encountered(Encountered::Element) {
                    next_event = Some(event);
                }
                self.namespaces.push(Vec::new());
                self.enter(
                    ParserState::InsideOpeningTag(OpeningTag::InsideName),
                    next_event,
                )
            }
            Token::ClosingTagStart if self.depth() > 0 => self.enter(
                ParserState::InsideClosingTag(ClosingTag::InsideName),
                next_event,
            ),
            Token::CommentStart => {
                if let Some(event) = self.set_encountered(Encountered::Comment) {
                    next_event = Some(event);
                }
                self.enter(ParserState::InsideComment, next_event)
            }
            Token::DoctypeStart if self.encountered < Encountered::Doctype => {
                if let Some(event) = self.set_encountered(Encountered::Doctype) {
                    next_event = Some(event);
                }
                self.doctype_length = DOCTYPE_MARKUP.len();
                self.push_position();
                self.enter(ParserState::InsideDoctype(Doctype::BeforeName), next_event)
            }
            Token::ProcessingInstructionStart => self.enter(
                ParserState::InsideProcessingInstruction(ProcessingInstruction::InsideName),
                next_event,
            ),
            Token::CDataStart if self.depth() > 0 => {
                self.enter(ParserState::InsideCData, next_event)
            }
            _ => Some(Err(ReaderError::UnexpectedToken)),
        }
    }

    fn document_start(&mut self, token: Token) -> Step {
        match token {
            Token::Character(character) => {
                let next_event = self.set_encountered(Encountered::AnyCharacters);
                if !is_whitespace(character) {
                    return Some(Err(ReaderError::UnexpectedTokenOutsideRoot));
                }
                self.enter(ParserState::OutsideTag, next_event)
            }
            Token::CommentStart => {
                let next_event = self.set_encountered(Encountered::Comment);
                self.enter(ParserState::InsideComment, next_event)
            }
            Token::OpeningTagStart => {
                let next_event = self.set_encountered(Encountered::Element);
                self.namespaces.push(Vec::new());
                self.enter(
                    ParserState::InsideOpeningTag(OpeningTag::InsideName),
                    next_event,
                )
            }
            Token::DoctypeStart => {
                let next_event = self.set_encountered(Encountered::Doctype);
                self.doctype_length = DOCTYPE_MARKUP.len();
                self.push_position();
                self.enter(ParserState::InsideDoctype(Doctype::BeforeName), next_event)
            }
            Token::ProcessingInstructionStart => {
                self.push_position();
                self.proceed(ParserState::InsideProcessingInstruction(
                    ProcessingInstruction::InsideName,
                ))
            }
            _ => Some(Err(ReaderError::UnexpectedToken)),
        }
    }

    fn read_qualified_name(&mut self, token: Token, target: NameTarget) -> NameStep {
        let completes = match token {
            Token::Character(':')
                if !self.qualified_name.is_empty() && !self.seen_prefix_separator =>
            {
                self.qualified_name.push(':');
                self.seen_prefix_separator = true;
                return NameStep::Pending;
            }
            Token::Character(character)
                if character != ':' && extends_name(&self.qualified_name, character) =>
            {
                if self.qualified_name.len() > MAX_NAME_LENGTH_BYTES {
                    return NameStep::Failed(ReaderError::LimitExceeded);
                }
                self.qualified_name.push(character);
                return NameStep::Pending;
            }
            Token::EqualsSign => target == NameTarget::Attribute,
            Token::EmptyTagEnd => target == NameTarget::OpeningTag,
            Token::TagEnd => target == NameTarget::OpeningTag || target == NameTarget::ClosingTag,
            Token::Character(character) => is_whitespace(character),
            _ => false,
        };
        if !completes {
            return NameStep::Failed(ReaderError::UnexpectedToken);
        }
        self.seen_prefix_separator = false;
        let parsed = Name::parse(&self.qualified_name);
        self.qualified_name.clear();
        match parsed {
            Some(name) => NameStep::Complete(token, name),
            None => NameStep::Failed(ReaderError::InvalidQualifiedName),
        }
    }

    fn read_attribute_value(&mut self, token: Token) -> ValueStep {
        match token {
            Token::Character(character) if self.quote.is_none() && is_whitespace(character) => {
                ValueStep::Pending
            }
            Token::DoubleQuote | Token::SingleQuote => match self.quote {
                None => {
                    self.quote = Quote::from_token(token);
                    ValueStep::Pending
                }
                Some(quote) if quote.token() == token => {
                    self.quote = None;
                    ValueStep::Complete(self.take_text())
                }
                Some(_) => self.push_attribute_token(token),
            },
            Token::ReferenceStart if self.quote.is_some() => {
                self.state_after_reference = self.state;
                self.state = ParserState::InsideReference;
                ValueStep::Pending
            }
            Token::OpeningTagStart | Token::ProcessingInstructionStart => {
                ValueStep::Failed(ReaderError::UnexpectedOpeningTag)
            }
            Token::Character(character) if !self.is_valid_unrestricted_character(character) => {
                ValueStep::Failed(ReaderError::InvalidCharacter)
            }
            _ if self.quote.is_some() => self.push_attribute_token(token),
            _ => ValueStep::Failed(ReaderError::UnexpectedToken),
        }
    }

    fn push_attribute_token(&mut self, token: Token) -> ValueStep {
        if self.text.len() > MAX_ATTRIBUTE_LENGTH_BYTES {
            return ValueStep::Failed(ReaderError::LimitExceeded);
        }
        token.push_to(&mut self.text);
        ValueStep::Pending
    }

    fn has_duplicate_attribute(attributes: &[(Name, String)]) -> bool {
        if attributes.len() >= ATTRIBUTES_HASHED_DUPLICATE_SCAN_FROM {
            let mut seen = HashSet::with_capacity(attributes.len());
            attributes.iter().any(|(name, _)| {
                !seen.insert((name.namespace.as_deref(), name.local_name.as_str()))
            })
        } else {
            attributes.iter().enumerate().any(|(index, (name, _))| {
                attributes[..index].iter().any(|(previous, _)| {
                    previous.namespace == name.namespace && previous.local_name == name.local_name
                })
            })
        }
    }

    fn emit_start_element(&mut self, empty: bool) -> Step {
        let Some(mut name) = self.element_name.take() else {
            return Some(Err(ReaderError::UnexpectedToken));
        };
        let mut attributes = core::mem::take(&mut self.attributes);
        name.namespace = match self.resolve(name.prefix.as_deref().unwrap_or(NO_PREFIX)) {
            Ok(namespace) => namespace,
            Err(error) => return Some(Err(error)),
        };
        for (attribute, _) in &mut attributes {
            if let Some(prefix) = attribute.prefix.as_deref() {
                attribute.namespace = match self.resolve(prefix) {
                    Ok(namespace) => namespace,
                    Err(error) => return Some(Err(error)),
                };
            }
        }
        if Self::has_duplicate_attribute(&attributes) {
            return Some(Err(ReaderError::RedefinedAttribute));
        }
        if empty {
            self.pop_namespace = true;
            self.next_event = Some(XmlEvent::EndElement {
                name: name.clone().qualified(),
            });
        } else {
            self.open_elements.push(name.clone());
        }
        let attributes = attributes
            .into_iter()
            .map(|(name, value)| Attribute {
                name: name.qualified(),
                value,
            })
            .collect();
        self.enter(
            ParserState::OutsideTag,
            Some(Ok(XmlEvent::StartElement {
                name: name.qualified(),
                attributes,
            })),
        )
    }

    fn emit_end_element(&mut self) -> Step {
        let Some(mut name) = self.element_name.take() else {
            return Some(Err(ReaderError::UnexpectedToken));
        };
        name.namespace = match self.resolve(name.prefix.as_deref().unwrap_or(NO_PREFIX)) {
            Ok(namespace) => namespace,
            Err(error) => return Some(Err(error)),
        };
        let Some(open) = self.open_elements.pop() else {
            return Some(Err(ReaderError::UnexpectedToken));
        };
        if name == open {
            self.pop_namespace = true;
            self.enter(
                ParserState::OutsideTag,
                Some(Ok(XmlEvent::EndElement {
                    name: name.qualified(),
                })),
            )
        } else {
            Some(Err(ReaderError::MismatchedClosingTag))
        }
    }

    fn inside_opening_tag(&mut self, token: Token, substate: OpeningTag) -> Step {
        match substate {
            OpeningTag::InsideName => match self.read_qualified_name(token, NameTarget::OpeningTag)
            {
                NameStep::Pending => None,
                NameStep::Failed(error) => Some(Err(error)),
                NameStep::Complete(_, name) if name.has_reserved_prefix() => {
                    Some(Err(ReaderError::InvalidNamePrefix))
                }
                NameStep::Complete(ending, name) => {
                    self.element_name = Some(name);
                    match ending {
                        Token::TagEnd => self.emit_start_element(false),
                        Token::EmptyTagEnd => self.emit_start_element(true),
                        _ => self.proceed(ParserState::InsideOpeningTag(OpeningTag::InsideTag)),
                    }
                }
            },
            OpeningTag::InsideTag => match token {
                Token::TagEnd => self.emit_start_element(false),
                Token::EmptyTagEnd => self.emit_start_element(true),
                Token::Character(character) if is_whitespace(character) => None,
                Token::Character(character) if is_name_start_character(character) => {
                    if self.qualified_name.len() > MAX_NAME_LENGTH_BYTES {
                        return Some(Err(ReaderError::LimitExceeded));
                    }
                    self.qualified_name.push(character);
                    self.proceed(ParserState::InsideOpeningTag(
                        OpeningTag::InsideAttributeName,
                    ))
                }
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            OpeningTag::InsideAttributeName => {
                match self.read_qualified_name(token, NameTarget::Attribute) {
                    NameStep::Pending => None,
                    NameStep::Failed(error) => Some(Err(error)),
                    NameStep::Complete(ending, name) => {
                        self.attribute_name = Some(name);
                        if ending == Token::EqualsSign {
                            self.proceed(ParserState::InsideOpeningTag(
                                OpeningTag::InsideAttributeValue,
                            ))
                        } else {
                            self.proceed(ParserState::InsideOpeningTag(
                                OpeningTag::AfterAttributeName,
                            ))
                        }
                    }
                }
            }
            OpeningTag::AfterAttributeName => match token {
                Token::EqualsSign => self.proceed(ParserState::InsideOpeningTag(
                    OpeningTag::InsideAttributeValue,
                )),
                Token::Character(character) if is_whitespace(character) => None,
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            OpeningTag::InsideAttributeValue => match self.read_attribute_value(token) {
                ValueStep::Pending => None,
                ValueStep::Failed(error) => Some(Err(error)),
                ValueStep::Complete(value) => self.finish_attribute(value),
            },
            OpeningTag::AfterAttributeValue => match token {
                Token::Character(character) if is_whitespace(character) => {
                    self.proceed(ParserState::InsideOpeningTag(OpeningTag::InsideTag))
                }
                Token::TagEnd => self.emit_start_element(false),
                Token::EmptyTagEnd => self.emit_start_element(true),
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
        }
    }

    fn finish_attribute(&mut self, value: String) -> Step {
        let Some(name) = self.attribute_name.take() else {
            return Some(Err(ReaderError::UnexpectedToken));
        };
        let after_value = ParserState::InsideOpeningTag(OpeningTag::AfterAttributeValue);
        match name.prefix.as_deref() {
            Some(XMLNS_PREFIX) => {
                if name.local_name == XMLNS_PREFIX
                    || (name.local_name == XML_PREFIX && value != XML_NAMESPACE_URI)
                    || value.is_empty()
                {
                    return Some(Err(ReaderError::InvalidNamespaceDeclaration));
                }
                self.bind_namespace(name.local_name, value);
                self.proceed(after_value)
            }
            None if name.local_name == XMLNS_PREFIX => match value.as_str() {
                XMLNS_PREFIX | XML_PREFIX | XML_NAMESPACE_URI | XMLNS_NAMESPACE_URI => {
                    Some(Err(ReaderError::InvalidNamespaceDeclaration))
                }
                _ => {
                    self.bind_namespace(NO_PREFIX.to_owned(), value);
                    self.proceed(after_value)
                }
            },
            _ => {
                if self.attributes.len() >= MAX_ATTRIBUTES {
                    return Some(Err(ReaderError::LimitExceeded));
                }
                self.attributes.push((name, value));
                self.proceed(after_value)
            }
        }
    }

    fn inside_closing_tag(&mut self, token: Token, substate: ClosingTag) -> Step {
        match substate {
            ClosingTag::InsideName => match self.read_qualified_name(token, NameTarget::ClosingTag)
            {
                NameStep::Pending => None,
                NameStep::Failed(error) => Some(Err(error)),
                NameStep::Complete(_, name) if name.has_reserved_prefix() => {
                    Some(Err(ReaderError::InvalidNamePrefix))
                }
                NameStep::Complete(ending, name) => {
                    self.element_name = Some(name);
                    match ending {
                        Token::TagEnd => self.emit_end_element(),
                        Token::Character(character) if is_whitespace(character) => {
                            self.proceed(ParserState::InsideClosingTag(ClosingTag::AfterName))
                        }
                        _ => Some(Err(ReaderError::UnexpectedToken)),
                    }
                }
            },
            ClosingTag::AfterName => match token {
                Token::TagEnd => self.emit_end_element(),
                Token::Character(character) if is_whitespace(character) => None,
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
        }
    }

    fn numeric_reference(&self, digits: &str) -> Result<char, ReaderError> {
        let value = match digits.strip_prefix('x') {
            Some(hexadecimal) => u32::from_str_radix(hexadecimal, HEXADECIMAL_RADIX),
            None => u32::from_str_radix(digits, DECIMAL_RADIX),
        }
        .map_err(|_| ReaderError::InvalidCharacterReference)?;
        match char::from_u32(value) {
            Some(character) if self.is_valid_character(character) => Ok(character),
            _ => Err(ReaderError::InvalidCharacterReference),
        }
    }

    fn inside_reference(&mut self, token: Token) -> Step {
        match token {
            Token::Character(character)
                if extends_name(&self.reference, character)
                    || (self.reference.is_empty() && character == '#') =>
            {
                self.reference.push(character);
                None
            }
            Token::ReferenceEnd => {
                let name = core::mem::take(&mut self.reference);
                if name.is_empty() {
                    return Some(Err(ReaderError::EmptyEntity));
                }
                let predefined = match name.as_str() {
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "amp" => Some('&'),
                    "apos" => Some('\''),
                    "quot" => Some('"'),
                    _ => match name.strip_prefix('#') {
                        Some(digits) => match self.numeric_reference(digits) {
                            Ok(character) => Some(character),
                            Err(error) => return Some(Err(error)),
                        },
                        None => None,
                    },
                };
                if let Some(character) = predefined {
                    self.text.push(character);
                } else if let Some(value) = self.entities.get(&name) {
                    if self.state_after_reference == ParserState::OutsideTag {
                        if let Err(error) = self.lexer.reparse(value) {
                            return Some(Err(error));
                        }
                    } else {
                        self.text.push_str(value);
                    }
                } else {
                    return Some(Err(ReaderError::UndefinedEntity));
                }
                self.proceed(self.state_after_reference)
            }
            _ => Some(Err(ReaderError::UnexpectedToken)),
        }
    }

    fn inside_comment(&mut self, token: Token) -> Step {
        match token {
            Token::CommentEnd => {
                let data = self.take_text();
                self.enter(ParserState::OutsideTag, Some(Ok(XmlEvent::Comment(data))))
            }
            Token::Character(character) if !self.is_valid_character(character) => {
                Some(Err(ReaderError::InvalidCharacter))
            }
            _ => self.push_data_token(token),
        }
    }

    fn push_data_token(&mut self, token: Token) -> Step {
        if self.text.len() > MAX_DATA_LENGTH_BYTES {
            return Some(Err(ReaderError::LimitExceeded));
        }
        token.push_to(&mut self.text);
        None
    }

    fn inside_cdata(&mut self, token: Token) -> Step {
        match token {
            Token::CDataEnd => {
                let data = self.take_text();
                self.enter(ParserState::OutsideTag, Some(Ok(XmlEvent::CData(data))))
            }
            Token::Character(character) if !self.is_valid_character(character) => {
                Some(Err(ReaderError::InvalidCharacter))
            }
            Token::Character(character) => {
                self.text.push(character);
                None
            }
            _ => None,
        }
    }

    fn inside_processing_instruction(
        &mut self,
        token: Token,
        substate: ProcessingInstruction,
    ) -> Step {
        match substate {
            ProcessingInstruction::InsideName => match token {
                Token::Character(character) if extends_name(&self.text, character) => {
                    if self.text.len() > MAX_NAME_LENGTH_BYTES {
                        return Some(Err(ReaderError::LimitExceeded));
                    }
                    self.text.push(character);
                    None
                }
                Token::ProcessingInstructionEnd => {
                    let name = self.take_text();
                    if name.is_empty() || name.eq_ignore_ascii_case(XML_PREFIX) {
                        return Some(Err(ReaderError::InvalidProcessingInstruction));
                    }
                    let instruction = XmlEvent::ProcessingInstruction { name, data: None };
                    match self.set_encountered(Encountered::Declaration) {
                        Some(start) => {
                            self.next_event = Some(instruction);
                            self.enter(ParserState::OutsideTag, Some(start))
                        }
                        None => self.enter(ParserState::OutsideTag, Some(Ok(instruction))),
                    }
                }
                Token::Character(character) if is_whitespace(character) => {
                    let name = self.take_text();
                    if name == XML_PREFIX && self.encountered == Encountered::Nothing {
                        return self
                            .proceed(ParserState::InsideDeclaration(Declaration::BeforeVersion));
                    }
                    if name.eq_ignore_ascii_case(XML_PREFIX) {
                        return Some(Err(ReaderError::InvalidProcessingInstruction));
                    }
                    self.markup_name = name;
                    let next_event = self.set_encountered(Encountered::Declaration);
                    self.enter(
                        ParserState::InsideProcessingInstruction(ProcessingInstruction::InsideData),
                        next_event,
                    )
                }
                _ => {
                    self.text.clear();
                    Some(Err(ReaderError::InvalidProcessingInstruction))
                }
            },
            ProcessingInstruction::InsideData => match token {
                Token::ProcessingInstructionEnd => {
                    let name = core::mem::take(&mut self.markup_name);
                    let data = self.take_text();
                    self.enter(
                        ParserState::OutsideTag,
                        Some(Ok(XmlEvent::ProcessingInstruction {
                            name,
                            data: Some(data),
                        })),
                    )
                }
                Token::Character(character) if !self.is_valid_character(character) => {
                    Some(Err(ReaderError::InvalidCharacter))
                }
                _ => self.push_data_token(token),
            },
        }
    }

    fn emit_start_document(&mut self) -> Step {
        self.encountered = Encountered::Declaration;
        if let Some(declared) = self.declared_encoding.take() {
            let Some(declared) = DeclaredEncoding::parse(&declared) else {
                return Some(Err(ReaderError::UnsupportedEncoding));
            };
            let current = self.lexer.decoder.encoding;
            let encoding = match (current, declared) {
                (Encoding::Utf8, DeclaredEncoding::Utf8)
                | (Encoding::Latin1, DeclaredEncoding::Latin1)
                | (Encoding::Ascii, DeclaredEncoding::Ascii)
                | (
                    Encoding::Utf16BigEndian | Encoding::Utf16LittleEndian,
                    DeclaredEncoding::Utf16,
                ) => current,
                (Encoding::Unknown | Encoding::Default, DeclaredEncoding::Utf8) => Encoding::Utf8,
                (Encoding::Unknown | Encoding::Default, DeclaredEncoding::Latin1) => {
                    Encoding::Latin1
                }
                (Encoding::Unknown | Encoding::Default, DeclaredEncoding::Ascii) => Encoding::Ascii,
                _ => return Some(Err(ReaderError::ConflictingEncoding)),
            };
            self.lexer.decoder.encoding = encoding;
        }
        self.enter(ParserState::OutsideTag, Some(Ok(XmlEvent::StartDocument)))
    }

    fn declaration_name(
        &mut self,
        token: Token,
        expected: &str,
        on_equals: Declaration,
        otherwise: Declaration,
    ) -> Step {
        match self.read_qualified_name(token, NameTarget::Attribute) {
            NameStep::Pending => None,
            NameStep::Failed(error) => Some(Err(error)),
            NameStep::Complete(ending, name) if name.local_name == expected => {
                let next = if ending == Token::EqualsSign {
                    on_equals
                } else {
                    otherwise
                };
                self.proceed(ParserState::InsideDeclaration(next))
            }
            NameStep::Complete(..) => Some(Err(ReaderError::UnexpectedToken)),
        }
    }

    fn inside_declaration(&mut self, token: Token, substate: Declaration) -> Step {
        let whitespace = matches!(token, Token::Character(character) if is_whitespace(character));
        let declaration = ParserState::InsideDeclaration;
        match substate {
            Declaration::BeforeVersion => match token {
                Token::Character('v') => self.proceed(declaration(Declaration::InsideVersion)),
                _ if whitespace => None,
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            Declaration::InsideVersion => self.declaration_name(
                token,
                "ersion",
                Declaration::InsideVersionValue,
                Declaration::AfterVersion,
            ),
            Declaration::AfterVersion => match token {
                Token::EqualsSign => self.proceed(declaration(Declaration::InsideVersionValue)),
                _ if whitespace => None,
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            Declaration::InsideVersionValue => match self.read_attribute_value(token) {
                ValueStep::Pending => None,
                ValueStep::Failed(error) => Some(Err(error)),
                ValueStep::Complete(value) => {
                    self.version = match value.as_str() {
                        "1.0" => Some(XmlVersion::Xml10),
                        "1.1" => Some(XmlVersion::Xml11),
                        other
                            if other.starts_with("1.")
                                && other.len() >= 3
                                && other
                                    .chars()
                                    .skip(2)
                                    .all(|character| character.is_ascii_digit()) =>
                        {
                            Some(XmlVersion::Xml10)
                        }
                        _ => None,
                    };
                    if self.version.is_some() {
                        self.proceed(declaration(Declaration::AfterVersionValue))
                    } else {
                        Some(Err(ReaderError::InvalidVersion))
                    }
                }
            },
            Declaration::AfterVersionValue => match token {
                _ if whitespace => self.proceed(declaration(Declaration::BeforeEncoding)),
                Token::ProcessingInstructionEnd => self.emit_start_document(),
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            Declaration::BeforeEncoding => match token {
                Token::Character('e') => self.proceed(declaration(Declaration::InsideEncoding)),
                Token::Character('s') => self.proceed(declaration(Declaration::InsideStandalone)),
                Token::ProcessingInstructionEnd => self.emit_start_document(),
                _ if whitespace => None,
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            Declaration::InsideEncoding => self.declaration_name(
                token,
                "ncoding",
                Declaration::InsideEncodingValue,
                Declaration::AfterEncoding,
            ),
            Declaration::AfterEncoding => match token {
                Token::EqualsSign => self.proceed(declaration(Declaration::InsideEncodingValue)),
                _ if whitespace => None,
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            Declaration::InsideEncodingValue => match self.read_attribute_value(token) {
                ValueStep::Pending => None,
                ValueStep::Failed(error) => Some(Err(error)),
                ValueStep::Complete(value) => {
                    self.declared_encoding = Some(value);
                    self.proceed(declaration(Declaration::AfterEncodingValue))
                }
            },
            Declaration::AfterEncodingValue => match token {
                _ if whitespace => self.proceed(declaration(Declaration::BeforeStandalone)),
                Token::ProcessingInstructionEnd => self.emit_start_document(),
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            Declaration::BeforeStandalone => match token {
                Token::Character('s') => self.proceed(declaration(Declaration::InsideStandalone)),
                Token::ProcessingInstructionEnd => self.emit_start_document(),
                _ if whitespace => None,
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            Declaration::InsideStandalone => self.declaration_name(
                token,
                "tandalone",
                Declaration::InsideStandaloneValue,
                Declaration::AfterStandalone,
            ),
            Declaration::AfterStandalone => match token {
                Token::EqualsSign => self.proceed(declaration(Declaration::InsideStandaloneValue)),
                _ if whitespace => None,
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
            Declaration::InsideStandaloneValue => match self.read_attribute_value(token) {
                ValueStep::Pending => None,
                ValueStep::Failed(error) => Some(Err(error)),
                ValueStep::Complete(value) if value == "yes" || value == "no" => {
                    self.proceed(declaration(Declaration::AfterStandaloneValue))
                }
                ValueStep::Complete(_) => Some(Err(ReaderError::InvalidStandalone)),
            },
            Declaration::AfterStandaloneValue => match token {
                Token::ProcessingInstructionEnd => self.emit_start_document(),
                _ if whitespace => None,
                _ => Some(Err(ReaderError::UnexpectedToken)),
            },
        }
    }

    fn quoted_literal(&mut self, token: Token, literal: Literal) -> Step {
        match token {
            Token::SingleQuote if self.quote != Some(Quote::Single) => {
                self.text.push('\'');
                None
            }
            Token::DoubleQuote if self.quote != Some(Quote::Double) => {
                self.text.push('"');
                None
            }
            Token::SingleQuote | Token::DoubleQuote => {
                self.quote = None;
                self.text.clear();
                let next = match literal {
                    Literal::System => Doctype::Outside,
                    Literal::Public => Doctype::BeforeSystemLiteral,
                };
                self.proceed(ParserState::InsideDoctype(next))
            }
            Token::Character(character)
                if literal == Literal::System || is_pubid_character(character) =>
            {
                self.text.push(character);
                None
            }
            Token::ReferenceEnd if literal == Literal::Public => {
                self.text.push(';');
                None
            }
            _ => Some(Err(ReaderError::UnexpectedToken)),
        }
    }

    fn lookup_parameter(&mut self) -> Result<String, ReaderError> {
        let name = core::mem::take(&mut self.reference);
        self.entities
            .get(&name)
            .cloned()
            .ok_or(ReaderError::UndefinedEntity)
    }

    fn push_markup_name(&mut self, character: char) -> Step {
        if self.markup_name.len() > MAX_NAME_LENGTH_BYTES {
            return Some(Err(ReaderError::LimitExceeded));
        }
        self.markup_name.push(character);
        None
    }

    fn inside_doctype(&mut self, token: Token, substate: Doctype) -> Step {
        self.doctype_length += token.displayed_length();
        if self.doctype_length > MAX_DATA_LENGTH_BYTES {
            return Some(Err(ReaderError::LimitExceeded));
        }
        let whitespace = matches!(token, Token::Character(character) if is_whitespace(character));
        let doctype = ParserState::InsideDoctype;
        let unexpected = Some(Err(ReaderError::UnexpectedToken));
        match substate {
            Doctype::BeforeName => match token {
                _ if whitespace => None,
                Token::Character(character) if is_name_start_character(character) => {
                    self.text.push(character);
                    self.proceed(doctype(Doctype::Name))
                }
                _ => unexpected,
            },
            Doctype::Name => match token {
                Token::TagEnd => {
                    self.text.clear();
                    self.enter(ParserState::OutsideTag, Some(Ok(XmlEvent::Doctype)))
                }
                _ if whitespace => {
                    self.text.clear();
                    self.proceed(doctype(Doctype::Outside))
                }
                Token::Character(character) if is_name_character(character) => {
                    self.text.push(character);
                    if self.text.len() > MAX_NAME_LENGTH_BYTES {
                        return Some(Err(ReaderError::LimitExceeded));
                    }
                    None
                }
                _ => unexpected,
            },
            Doctype::ExternalIdKeyword => match token {
                Token::Character(character @ 'A'..='Z') => {
                    self.text.push(character);
                    if self.text == SYSTEM_KEYWORD {
                        self.text.clear();
                        return self.proceed(doctype(Doctype::BeforeSystemLiteral));
                    }
                    if self.text == PUBLIC_KEYWORD {
                        self.text.clear();
                        return self.proceed(doctype(Doctype::BeforePublicId));
                    }
                    if PUBLIC_KEYWORD.starts_with(self.text.as_str())
                        || SYSTEM_KEYWORD.starts_with(self.text.as_str())
                    {
                        return None;
                    }
                    unexpected
                }
                _ => unexpected,
            },
            Doctype::BeforeSystemLiteral | Doctype::BeforePublicId => match token {
                _ if whitespace => None,
                Token::SingleQuote | Token::DoubleQuote => {
                    self.quote = Quote::from_token(token);
                    self.text.clear();
                    let literal = if substate == Doctype::BeforeSystemLiteral {
                        Doctype::SystemLiteral
                    } else {
                        Doctype::PublicId
                    };
                    self.proceed(doctype(literal))
                }
                _ => unexpected,
            },
            Doctype::SystemLiteral => self.quoted_literal(token, Literal::System),
            Doctype::PublicId => self.quoted_literal(token, Literal::Public),
            Doctype::Outside => match token {
                Token::TagEnd => self.enter(ParserState::OutsideTag, Some(Ok(XmlEvent::Doctype))),
                Token::Character('[') => self.proceed(doctype(Doctype::InternalSubset)),
                Token::Character(character @ ('S' | 'P')) => {
                    self.text.push(character);
                    self.proceed(doctype(Doctype::ExternalIdKeyword))
                }
                _ if whitespace => None,
                _ => unexpected,
            },
            Doctype::InternalSubset => match token {
                Token::Character(']') => self.proceed(doctype(Doctype::Outside)),
                Token::SingleQuote | Token::DoubleQuote => {
                    self.quote = Quote::from_token(token);
                    self.proceed(doctype(Doctype::String))
                }
                Token::CommentStart => self.proceed(doctype(Doctype::Comment)),
                Token::Character('%') => {
                    self.reference.clear();
                    self.reference.push('%');
                    self.proceed(doctype(Doctype::ParameterReferenceInSubset))
                }
                Token::MarkupDeclarationStart => {
                    self.text.clear();
                    self.proceed(doctype(Doctype::InsideName))
                }
                _ if whitespace => None,
                Token::ProcessingInstructionStart => {
                    self.proceed(doctype(Doctype::IgnoredProcessingInstruction))
                }
                _ => unexpected,
            },
            Doctype::IgnoredProcessingInstruction => match token {
                Token::ProcessingInstructionEnd => self.proceed(doctype(Doctype::InternalSubset)),
                _ => None,
            },
            Doctype::String => match token {
                Token::SingleQuote if self.quote != Some(Quote::Single) => None,
                Token::DoubleQuote if self.quote != Some(Quote::Double) => None,
                Token::SingleQuote | Token::DoubleQuote => {
                    self.quote = None;
                    self.proceed(doctype(Doctype::InternalSubset))
                }
                _ => None,
            },
            Doctype::Comment => match token {
                Token::CommentEnd => self.proceed(doctype(Doctype::InternalSubset)),
                _ => None,
            },
            Doctype::InsideName => match token {
                Token::Character(character @ 'A'..='Z') => {
                    self.text.push(character);
                    None
                }
                _ if whitespace => {
                    let next = match self.text.as_str() {
                        "ENTITY" => Doctype::BeforeEntityName,
                        "NOTATION" | "ELEMENT" | "ATTLIST" => Doctype::SkippedDeclaration,
                        _ => return Some(Err(ReaderError::UnknownMarkupDeclaration)),
                    };
                    self.text.clear();
                    self.proceed(doctype(next))
                }
                _ => unexpected,
            },
            Doctype::BeforeEntityName => {
                self.markup_name.clear();
                match token {
                    _ if whitespace => None,
                    Token::Character('%') => {
                        self.markup_name.push('%');
                        self.proceed(doctype(Doctype::ParameterDefinitionStart))
                    }
                    Token::Character(character) if is_name_start_character(character) => {
                        self.markup_name.push(character);
                        self.proceed(doctype(Doctype::EntityName))
                    }
                    _ => unexpected,
                }
            }
            Doctype::EntityName | Doctype::ParameterDefinition => match token {
                _ if whitespace => self.proceed(doctype(Doctype::BeforeEntityValue)),
                Token::Character(character) if is_name_character(character) => {
                    self.push_markup_name(character)
                }
                _ => unexpected,
            },
            Doctype::BeforeEntityValue => {
                self.text.clear();
                match token {
                    _ if whitespace => None,
                    Token::Character('S' | 'P') => {
                        let name = core::mem::take(&mut self.markup_name);
                        self.entities.entry(name).or_default();
                        self.proceed(doctype(Doctype::SkippedDeclaration))
                    }
                    Token::SingleQuote | Token::DoubleQuote => {
                        self.quote = Quote::from_token(token);
                        self.proceed(doctype(Doctype::EntityValue))
                    }
                    _ => unexpected,
                }
            }
            Doctype::EntityValue => match token {
                Token::SingleQuote if self.quote != Some(Quote::Single) => {
                    self.text.push('\'');
                    None
                }
                Token::DoubleQuote if self.quote != Some(Quote::Double) => {
                    self.text.push('"');
                    None
                }
                Token::SingleQuote | Token::DoubleQuote => {
                    self.quote = None;
                    let name = core::mem::take(&mut self.markup_name);
                    let value = self.take_text();
                    self.entities.entry(name).or_insert(value);
                    self.proceed(doctype(Doctype::SkippedDeclaration))
                }
                Token::ReferenceStart | Token::Character('&') => {
                    self.reference.clear();
                    self.proceed(doctype(Doctype::NumericReferenceStart))
                }
                Token::Character('%') => {
                    self.reference.clear();
                    self.reference.push('%');
                    self.proceed(doctype(Doctype::ParameterReferenceInValue))
                }
                Token::Character(character) if !self.is_valid_character(character) => {
                    Some(Err(ReaderError::InvalidCharacter))
                }
                Token::Character(character) => {
                    self.text.push(character);
                    None
                }
                _ => unexpected,
            },
            Doctype::ParameterDefinitionStart => match token {
                _ if whitespace => None,
                Token::Character(character) if is_name_start_character(character) => {
                    self.markup_name.push(character);
                    self.proceed(doctype(Doctype::ParameterDefinition))
                }
                _ => unexpected,
            },
            Doctype::ParameterReferenceInSubset | Doctype::ParameterReferenceInValue => match token
            {
                Token::Character(character) if is_name_character(character) => {
                    self.reference.push(character);
                    None
                }
                Token::ReferenceEnd | Token::Character(';') => {
                    let value = match self.lookup_parameter() {
                        Ok(value) => value,
                        Err(error) => return Some(Err(error)),
                    };
                    if substate == Doctype::ParameterReferenceInSubset {
                        if let Err(error) = self.lexer.reparse(&value) {
                            return Some(Err(error));
                        }
                        self.proceed(doctype(Doctype::InternalSubset))
                    } else {
                        self.text.push_str(&value);
                        self.proceed(doctype(Doctype::EntityValue))
                    }
                }
                _ => unexpected,
            },
            Doctype::NumericReferenceStart => match token {
                Token::Character('#') => self.proceed(doctype(Doctype::NumericReference)),
                Token::Character(character) if !self.is_valid_character(character) => {
                    Some(Err(ReaderError::InvalidCharacter))
                }
                Token::Character(character) => {
                    self.text.push('&');
                    self.text.push(character);
                    self.proceed(doctype(Doctype::EntityValue))
                }
                _ => unexpected,
            },
            Doctype::NumericReference => match token {
                Token::ReferenceEnd | Token::Character(';') => {
                    let digits = core::mem::take(&mut self.reference);
                    match self.numeric_reference(&digits) {
                        Ok(character) => {
                            self.text.push(character);
                            self.proceed(doctype(Doctype::EntityValue))
                        }
                        Err(error) => Some(Err(error)),
                    }
                }
                Token::Character(character) if !self.is_valid_character(character) => {
                    Some(Err(ReaderError::InvalidCharacter))
                }
                Token::Character(character) => {
                    self.reference.push(character);
                    None
                }
                _ => unexpected,
            },
            Doctype::SkippedDeclaration => match token {
                Token::TagEnd => self.proceed(doctype(Doctype::InternalSubset)),
                _ => None,
            },
        }
    }
}

pub(crate) struct EventReader<R> {
    source: R,
    parser: Parser,
}

impl<R: Read> EventReader<R> {
    pub(crate) fn new(source: R) -> Self {
        Self {
            source,
            parser: Parser::new(),
        }
    }

    pub(crate) fn next(&mut self) -> Result<XmlEvent, ReaderError> {
        self.parser.next(&mut self.source)
    }

    pub(crate) fn position(&self) -> TextPosition {
        self.parser.position
    }
}
