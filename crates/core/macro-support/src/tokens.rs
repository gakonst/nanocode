use std::borrow::Cow;
use std::fmt;
use std::ops::BitOr;
use std::rc::Rc;
use std::str::FromStr;

use proc_macro::{
    Delimiter, Group, Ident as RawIdent, Literal, Punct, Spacing, Span, TokenStream, TokenTree,
};

use crate::ident::Ident;

#[derive(Clone, Default)]
pub struct Tokens {
    trees: Vec<TokenTree>,
}

impl Tokens {
    pub const fn new() -> Self {
        Self { trees: Vec::new() }
    }

    pub fn from_stream(stream: TokenStream) -> Self {
        Self {
            trees: stream.into_iter().collect(),
        }
    }

    pub const fn from_trees(trees: Vec<TokenTree>) -> Self {
        Self { trees }
    }

    pub fn push(&mut self, tree: impl Into<TokenTree>) {
        self.trees.push(tree.into());
    }

    pub fn append(&mut self, tree: impl Into<TokenTree>) {
        self.push(tree);
    }

    pub fn append_all<I>(&mut self, items: I)
    where
        I: IntoIterator,
        I::Item: ToTokens,
    {
        for item in items {
            item.to_tokens(self);
        }
    }

    pub fn append_separated<I, S>(&mut self, items: I, separator: S)
    where
        I: IntoIterator,
        I::Item: ToTokens,
        S: ToTokens,
    {
        for (index, item) in items.into_iter().enumerate() {
            if index > 0 {
                separator.to_tokens(self);
            }
            item.to_tokens(self);
        }
    }

    pub fn extend(&mut self, other: Self) {
        self.trees.extend(other.trees);
    }

    pub const fn is_empty(&self) -> bool {
        self.trees.is_empty()
    }

    pub fn trees(&self) -> &[TokenTree] {
        &self.trees
    }

    pub fn into_trees(self) -> Vec<TokenTree> {
        self.trees
    }

    pub fn into_stream(self) -> TokenStream {
        self.trees.into_iter().collect()
    }

    pub fn first_span(&self) -> Option<Span> {
        self.trees.first().map(TokenTree::span)
    }

    pub fn last_span(&self) -> Option<Span> {
        self.trees.last().map(TokenTree::span)
    }
}

impl From<TokenStream> for Tokens {
    fn from(stream: TokenStream) -> Self {
        Self::from_stream(stream)
    }
}

impl From<Tokens> for TokenStream {
    fn from(tokens: Tokens) -> Self {
        tokens.into_stream()
    }
}

impl FromIterator<TokenTree> for Tokens {
    fn from_iter<I: IntoIterator<Item = TokenTree>>(iter: I) -> Self {
        Self {
            trees: iter.into_iter().collect(),
        }
    }
}

impl FromIterator<Self> for Tokens {
    fn from_iter<I: IntoIterator<Item = Self>>(iter: I) -> Self {
        let mut tokens = Self::new();
        for item in iter {
            tokens.extend(item);
        }
        tokens
    }
}

impl IntoIterator for Tokens {
    type Item = TokenTree;
    type IntoIter = std::vec::IntoIter<TokenTree>;

    fn into_iter(self) -> Self::IntoIter {
        self.trees.into_iter()
    }
}

impl fmt::Display for Tokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stream: TokenStream = self.trees.iter().cloned().collect();
        fmt::Display::fmt(&stream, formatter)
    }
}

impl fmt::Debug for Tokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

pub trait ToTokens {
    fn to_tokens(&self, tokens: &mut Tokens);

    fn to_token_stream(&self) -> Tokens {
        let mut tokens = Tokens::new();
        self.to_tokens(&mut tokens);
        tokens
    }

    fn into_token_stream(self) -> Tokens
    where
        Self: Sized,
    {
        self.to_token_stream()
    }
}

impl<T: ToTokens + ?Sized> ToTokens for &T {
    fn to_tokens(&self, tokens: &mut Tokens) {
        (**self).to_tokens(tokens);
    }
}

impl<T: ToTokens + ?Sized> ToTokens for &mut T {
    fn to_tokens(&self, tokens: &mut Tokens) {
        (**self).to_tokens(tokens);
    }
}

impl<T: ToTokens + ?Sized> ToTokens for Box<T> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        (**self).to_tokens(tokens);
    }
}

impl<T: ToTokens + ?Sized> ToTokens for Rc<T> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        (**self).to_tokens(tokens);
    }
}

impl<T: ToOwned + ToTokens + ?Sized> ToTokens for Cow<'_, T> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        (**self).to_tokens(tokens);
    }
}

impl<T: ToTokens> ToTokens for Option<T> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        if let Some(value) = self {
            value.to_tokens(tokens);
        }
    }
}

impl ToTokens for Tokens {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.trees.extend(self.trees.iter().cloned());
    }

    fn into_token_stream(self) -> Tokens {
        self
    }
}

impl ToTokens for TokenStream {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.trees.extend(self.clone());
    }
}

impl ToTokens for TokenTree {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(self.clone());
    }
}

impl ToTokens for Group {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(self.clone());
    }
}

impl ToTokens for RawIdent {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(self.clone());
    }
}

impl ToTokens for Punct {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(self.clone());
    }
}

impl ToTokens for Literal {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(self.clone());
    }
}

impl ToTokens for str {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(Literal::string(self));
    }
}

impl ToTokens for String {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.as_str().to_tokens(tokens);
    }
}

impl ToTokens for char {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(Literal::character(*self));
    }
}

impl ToTokens for bool {
    fn to_tokens(&self, tokens: &mut Tokens) {
        let word = if *self { "true" } else { "false" };
        tokens.push(RawIdent::new(word, Span::call_site()));
    }
}

macro_rules! suffixed_literals {
    ($($number:ty => $constructor:ident,)*) => {
        $(
            impl ToTokens for $number {
                fn to_tokens(&self, tokens: &mut Tokens) {
                    tokens.push(Literal::$constructor(*self));
                }
            }
        )*
    };
}

suffixed_literals! {
    i8 => i8_suffixed,
    i16 => i16_suffixed,
    i32 => i32_suffixed,
    i64 => i64_suffixed,
    i128 => i128_suffixed,
    isize => isize_suffixed,
    u8 => u8_suffixed,
    u16 => u16_suffixed,
    u32 => u32_suffixed,
    u64 => u64_suffixed,
    u128 => u128_suffixed,
    usize => usize_suffixed,
    f32 => f32_suffixed,
    f64 => f64_suffixed,
}

pub trait Spanned {
    fn span(&self) -> Span;
}

impl<T: ToTokens + ?Sized> Spanned for T {
    fn span(&self) -> Span {
        self.to_token_stream()
            .first_span()
            .unwrap_or_else(Span::call_site)
    }
}

pub trait IdentFragment {
    fn fragment(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result;

    fn span(&self) -> Option<Span> {
        None
    }
}

impl<T: IdentFragment + ?Sized> IdentFragment for &T {
    fn fragment(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fragment(formatter)
    }

    fn span(&self) -> Option<Span> {
        (**self).span()
    }
}

impl IdentFragment for Ident {
    fn fragment(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.unraw())
    }

    fn span(&self) -> Option<Span> {
        Some(Self::span(self))
    }
}

impl IdentFragment for RawIdent {
    fn fragment(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.to_string();
        formatter.write_str(text.strip_prefix("r#").unwrap_or(&text))
    }

    fn span(&self) -> Option<Span> {
        Some(Self::span(self))
    }
}

impl IdentFragment for str {
    fn fragment(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self)
    }
}

impl IdentFragment for String {
    fn fragment(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self)
    }
}

impl IdentFragment for Cow<'_, str> {
    fn fragment(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self)
    }
}

macro_rules! display_fragments {
    ($($kind:ty),*) => {
        $(
            impl IdentFragment for $kind {
                fn fragment(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    fmt::Display::fmt(self, formatter)
                }
            }
        )*
    };
}

display_fragments!(
    bool, char, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize
);

pub struct FragmentAdapter<'a, T: IdentFragment + ?Sized>(pub &'a T);

impl<T: IdentFragment + ?Sized> fmt::Display for FragmentAdapter<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fragment(formatter)
    }
}

impl<T: IdentFragment + ?Sized> FragmentAdapter<'_, T> {
    pub fn span(&self) -> Option<Span> {
        self.0.span()
    }
}

pub fn make_ident(text: &str, span: Option<Span>) -> Ident {
    Ident::new(text, span.unwrap_or_else(Span::call_site))
}

pub fn push_group(tokens: &mut Tokens, span: Span, delimiter: Delimiter, inner: Tokens) {
    let mut group = Group::new(delimiter, inner.into_stream());
    group.set_span(span);
    tokens.push(group);
}

pub fn push_ident(tokens: &mut Tokens, span: Span, text: &str) {
    match text.strip_prefix("r#") {
        Some(raw) => tokens.push(RawIdent::new_raw(raw, span)),
        None => tokens.push(RawIdent::new(text, span)),
    }
}

pub fn push_lifetime(tokens: &mut Tokens, span: Span, text: &str) {
    let name = text
        .strip_prefix('\'')
        .expect("a lifetime token starts with an apostrophe");
    let mut apostrophe = Punct::new('\'', Spacing::Joint);
    apostrophe.set_span(span);
    tokens.push(apostrophe);
    push_ident(tokens, span, name);
}

pub fn push_punct(tokens: &mut Tokens, span: Span, text: &str) {
    let count = text.chars().count();
    for (index, character) in text.chars().enumerate() {
        let spacing = if index + 1 == count {
            Spacing::Alone
        } else {
            Spacing::Joint
        };
        let mut punct = Punct::new(character, spacing);
        punct.set_span(span);
        tokens.push(punct);
    }
}

const PUNCTUATION_CHARACTERS: &str = "=<>!~+-*/%^&|@.,;:#$?";

pub fn push_other(tokens: &mut Tokens, span: Span, text: &str) {
    let first = text.chars().next().expect("a token has text");
    if PUNCTUATION_CHARACTERS.contains(first) {
        push_punct(tokens, span, text);
        return;
    }
    let mut literal = Literal::from_str(text).expect("a quoted literal token parses as a literal");
    literal.set_span(span);
    tokens.push(literal);
}

#[derive(Copy, Clone)]
pub struct HasIterator;

#[derive(Copy, Clone)]
pub struct ThereIsNoIteratorInRepetition;

impl BitOr<Self> for ThereIsNoIteratorInRepetition {
    type Output = Self;
    fn bitor(self, _other: Self) -> Self {
        Self
    }
}

impl BitOr<ThereIsNoIteratorInRepetition> for HasIterator {
    type Output = Self;
    fn bitor(self, _other: ThereIsNoIteratorInRepetition) -> Self {
        Self
    }
}

impl BitOr<HasIterator> for ThereIsNoIteratorInRepetition {
    type Output = HasIterator;
    fn bitor(self, _other: HasIterator) -> HasIterator {
        HasIterator
    }
}

impl BitOr<Self> for HasIterator {
    type Output = Self;
    fn bitor(self, _other: Self) -> Self {
        Self
    }
}

#[derive(Copy, Clone)]
pub struct RepInterp<T>(pub T);

impl<T> RepInterp<T> {
    #[allow(clippy::should_implement_trait)]
    pub fn next(self) -> Option<T> {
        Some(self.0)
    }
}

impl<T: Iterator> Iterator for RepInterp<T> {
    type Item = T::Item;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

impl<T: ToTokens> ToTokens for RepInterp<T> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.0.to_tokens(tokens);
    }
}

pub mod ext {
    use std::collections::btree_set::{self, BTreeSet};
    use std::slice;

    use super::{HasIterator, RepInterp, ThereIsNoIteratorInRepetition, ToTokens};

    pub trait RepIteratorExt: Iterator + Sized {
        fn quote_into_iter(self) -> (Self, HasIterator) {
            (self, HasIterator)
        }
    }

    impl<T: Iterator> RepIteratorExt for T {}

    pub trait RepToTokensExt {
        fn next(&self) -> Option<&Self> {
            Some(self)
        }

        fn quote_into_iter(&self) -> (&Self, ThereIsNoIteratorInRepetition) {
            (self, ThereIsNoIteratorInRepetition)
        }
    }

    impl<T: ToTokens + ?Sized> RepToTokensExt for T {}

    pub trait RepAsIteratorExt<'q> {
        type Iter: Iterator;
        fn quote_into_iter(&'q self) -> (Self::Iter, HasIterator);
    }

    impl<'q, T: RepAsIteratorExt<'q> + ?Sized> RepAsIteratorExt<'q> for &T {
        type Iter = T::Iter;
        fn quote_into_iter(&'q self) -> (Self::Iter, HasIterator) {
            <T as RepAsIteratorExt>::quote_into_iter(*self)
        }
    }

    impl<'q, T: RepAsIteratorExt<'q> + ?Sized> RepAsIteratorExt<'q> for &mut T {
        type Iter = T::Iter;
        fn quote_into_iter(&'q self) -> (Self::Iter, HasIterator) {
            <T as RepAsIteratorExt>::quote_into_iter(*self)
        }
    }

    impl<'q, T: 'q> RepAsIteratorExt<'q> for [T] {
        type Iter = slice::Iter<'q, T>;
        fn quote_into_iter(&'q self) -> (Self::Iter, HasIterator) {
            (self.iter(), HasIterator)
        }
    }

    impl<'q, T: 'q, const N: usize> RepAsIteratorExt<'q> for [T; N] {
        type Iter = slice::Iter<'q, T>;
        fn quote_into_iter(&'q self) -> (Self::Iter, HasIterator) {
            (self.iter(), HasIterator)
        }
    }

    impl<'q, T: 'q> RepAsIteratorExt<'q> for Vec<T> {
        type Iter = slice::Iter<'q, T>;
        fn quote_into_iter(&'q self) -> (Self::Iter, HasIterator) {
            (self.iter(), HasIterator)
        }
    }

    impl<'q, T: 'q> RepAsIteratorExt<'q> for BTreeSet<T> {
        type Iter = btree_set::Iter<'q, T>;
        fn quote_into_iter(&'q self) -> (Self::Iter, HasIterator) {
            (self.iter(), HasIterator)
        }
    }

    impl<'q, T: RepAsIteratorExt<'q>> RepAsIteratorExt<'q> for RepInterp<T> {
        type Iter = T::Iter;
        fn quote_into_iter(&'q self) -> (Self::Iter, HasIterator) {
            self.0.quote_into_iter()
        }
    }
}
