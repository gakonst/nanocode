use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use proc_macro::{Ident as RawIdent, Punct, Spacing, Span};

use crate::buffer::{Parse, ParseStream};
use crate::error::{Error, Result};
use crate::tokens::{ToTokens, Tokens};

#[derive(Clone)]
pub struct Ident {
    raw: RawIdent,
    text: Rc<str>,
}

impl Ident {
    pub fn new(text: &str, span: Span) -> Self {
        let raw = match text.strip_prefix("r#") {
            Some(name) => RawIdent::new_raw(name, span),
            None => RawIdent::new(text, span),
        };
        Self {
            raw,
            text: Rc::from(text),
        }
    }

    pub fn from_raw(raw: RawIdent) -> Self {
        let text = Rc::from(raw.to_string());
        Self { raw, text }
    }

    pub fn span(&self) -> Span {
        self.raw.span()
    }

    pub fn set_span(&mut self, span: Span) {
        self.raw.set_span(span);
    }

    pub const fn raw(&self) -> &RawIdent {
        &self.raw
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn unraw(&self) -> &str {
        self.text.strip_prefix("r#").unwrap_or(&self.text)
    }

    pub fn is_keyword(&self) -> bool {
        is_keyword(&self.text)
    }
}

const KEYWORDS: [&str; 51] = [
    "_", "abstract", "as", "async", "await", "become", "box", "break", "const", "continue",
    "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "Self", "self", "static", "struct", "super", "trait", "true", "try", "type",
    "typeof", "unsafe", "unsized", "use", "virtual", "where", "while",
];

pub fn is_keyword(text: &str) -> bool {
    text == "yield" || text == "gen" || KEYWORDS.contains(&text)
}

impl PartialEq for Ident {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for Ident {}

impl<T: AsRef<str> + ?Sized> PartialEq<T> for Ident {
    fn eq(&self, other: &T) -> bool {
        *self.text == *other.as_ref()
    }
}

impl Hash for Ident {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state);
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        self.text.cmp(&other.text)
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl fmt::Debug for Ident {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Ident({})", self.text)
    }
}

impl ToTokens for Ident {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(self.raw.clone());
    }
}

impl From<RawIdent> for Ident {
    fn from(raw: RawIdent) -> Self {
        Self::from_raw(raw)
    }
}

impl Parse for Ident {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let ident = input.parse_any_ident()?;
        if ident.is_keyword() && !matches!(ident.text(), "self" | "Self" | "super" | "crate") {
            return Err(Error::new(
                ident.span(),
                format!("expected identifier, found keyword `{ident}`"),
            ));
        }
        Ok(ident)
    }
}

#[derive(Clone)]
pub struct Lifetime {
    pub apostrophe: Span,
    pub ident: Ident,
}

impl Lifetime {
    pub fn new(text: &str, span: Span) -> Self {
        let name = text
            .strip_prefix('\'')
            .expect("a lifetime starts with an apostrophe");
        Self {
            apostrophe: span,
            ident: Ident::new(name, span),
        }
    }

    pub const fn span(&self) -> Span {
        self.apostrophe
    }
}

impl PartialEq for Lifetime {
    fn eq(&self, other: &Self) -> bool {
        self.ident == other.ident
    }
}

impl Eq for Lifetime {}

impl Hash for Lifetime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ident.hash(state);
    }
}

impl PartialOrd for Lifetime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Lifetime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ident.cmp(&other.ident)
    }
}

impl fmt::Display for Lifetime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "'{}", self.ident)
    }
}

impl fmt::Debug for Lifetime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Lifetime('{})", self.ident)
    }
}

impl ToTokens for Lifetime {
    fn to_tokens(&self, tokens: &mut Tokens) {
        let mut apostrophe = Punct::new('\'', Spacing::Joint);
        apostrophe.set_span(self.apostrophe);
        tokens.push(apostrophe);
        self.ident.to_tokens(tokens);
    }
}

impl Parse for Lifetime {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        input.parse_lifetime()
    }
}
