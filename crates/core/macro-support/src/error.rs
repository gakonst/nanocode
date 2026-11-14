use std::fmt;

use proc_macro::{
    Delimiter, Group, Ident as RawIdent, Literal, Punct, Spacing, Span, TokenStream, TokenTree,
};

use crate::tokens::{ToTokens, Tokens};

#[derive(Clone)]
struct Message {
    start: Span,
    end: Span,
    text: String,
}

#[derive(Clone)]
pub struct Error {
    messages: Vec<Message>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(span: Span, message: impl fmt::Display) -> Self {
        Self {
            messages: vec![Message {
                start: span,
                end: span,
                text: message.to_string(),
            }],
        }
    }

    pub fn new_spanned(tokens: impl ToTokens, message: impl fmt::Display) -> Self {
        let tokens = tokens.to_token_stream();
        let start = tokens.first_span().unwrap_or_else(Span::call_site);
        let end = tokens.last_span().unwrap_or(start);
        Self {
            messages: vec![Message {
                start,
                end,
                text: message.to_string(),
            }],
        }
    }

    pub fn span(&self) -> Span {
        self.messages[0].start
    }

    pub fn combine(&mut self, other: Self) {
        self.messages.extend(other.messages);
    }

    pub fn to_compile_error(&self) -> Tokens {
        let mut tokens = Tokens::new();
        for message in &self.messages {
            compile_error_tokens(message, &mut tokens);
        }
        tokens
    }

    pub fn into_compile_error(self) -> Tokens {
        self.to_compile_error()
    }

    pub fn messages(&self) -> impl Iterator<Item = &str> {
        self.messages.iter().map(|message| message.text.as_str())
    }
}

fn spanned_punct(character: char, spacing: Spacing, span: Span) -> Punct {
    let mut punct = Punct::new(character, spacing);
    punct.set_span(span);
    punct
}

fn compile_error_tokens(message: &Message, tokens: &mut Tokens) {
    let start = message.start;
    tokens.push(spanned_punct(':', Spacing::Joint, start));
    tokens.push(spanned_punct(':', Spacing::Alone, start));
    tokens.push(RawIdent::new("core", start));
    tokens.push(spanned_punct(':', Spacing::Joint, start));
    tokens.push(spanned_punct(':', Spacing::Alone, start));
    tokens.push(RawIdent::new("compile_error", start));
    tokens.push(spanned_punct('!', Spacing::Alone, start));
    let mut literal = Literal::string(&message.text);
    literal.set_span(message.end);
    let mut group = Group::new(
        Delimiter::Brace,
        TokenStream::from(TokenTree::Literal(literal)),
    );
    group.set_span(message.end);
    tokens.push(group);
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.messages[0].text)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.messages.iter().map(|message| &message.text))
            .finish()
    }
}

impl Extend<Self> for Error {
    fn extend<I: IntoIterator<Item = Self>>(&mut self, iter: I) {
        for error in iter {
            self.combine(error);
        }
    }
}
