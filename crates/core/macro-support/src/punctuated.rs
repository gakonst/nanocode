use std::ops::{Index, IndexMut};

use proc_macro::Span;

use crate::buffer::ParseStream;
use crate::error::Result;
use crate::tokens::{ToTokens, Tokens, push_punct};

pub trait Token: Clone + Default + ToTokens {
    fn peek(input: ParseStream<'_>) -> bool;
    fn parse_token(input: ParseStream<'_>) -> Result<Self>;
}

macro_rules! punctuation {
    ($($name:ident => $text:literal,)*) => {
        $(
            #[derive(Clone, Copy)]
            pub struct $name {
                pub span: Span,
            }

            impl Default for $name {
                fn default() -> Self {
                    Self { span: Span::call_site() }
                }
            }

            impl ToTokens for $name {
                fn to_tokens(&self, tokens: &mut Tokens) {
                    push_punct(tokens, self.span, $text);
                }
            }

            impl crate::buffer::Parse for $name {
                fn parse(input: ParseStream<'_>) -> Result<Self> {
                    input.parse_joint($text).map(|span| Self { span })
                }
            }

            impl Token for $name {
                fn peek(input: ParseStream<'_>) -> bool {
                    input.peek_joint($text)
                }

                fn parse_token(input: ParseStream<'_>) -> Result<Self> {
                    input.parse_joint($text).map(|span| Self { span })
                }
            }
        )*
    };
}

punctuation! {
    Comma => ",",
    Plus => "+",
    PathSep => "::",
    Colon => ":",
    Eq => "=",
    Semi => ";",
    Lt => "<",
    Gt => ">",
    Pound => "#",
    Not => "!",
    And => "&",
    Star => "*",
    RArrow => "->",
    Or => "|",
    Dot => ".",
}

#[derive(Clone)]
pub struct Punctuated<T, P> {
    inner: Vec<(T, P)>,
    last: Option<Box<T>>,
}

pub enum Pair<T, P> {
    Punctuated(T, P),
    End(T),
}

impl<T, P> Pair<T, P> {
    pub fn into_value(self) -> T {
        match self {
            Self::Punctuated(value, _) | Self::End(value) => value,
        }
    }

    pub const fn value(&self) -> &T {
        match self {
            Self::Punctuated(value, _) | Self::End(value) => value,
        }
    }

    pub const fn punct(&self) -> Option<&P> {
        match self {
            Self::Punctuated(_, punct) => Some(punct),
            Self::End(_) => None,
        }
    }

    pub fn into_tuple(self) -> (T, Option<P>) {
        match self {
            Self::Punctuated(value, punct) => (value, Some(punct)),
            Self::End(value) => (value, None),
        }
    }
}

impl<T: ToTokens, P: ToTokens> ToTokens for Pair<T, P> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Punctuated(value, punct) => {
                value.to_tokens(tokens);
                punct.to_tokens(tokens);
            }
            Self::End(value) => value.to_tokens(tokens),
        }
    }
}

impl<T, P> Default for Punctuated<T, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, P> Punctuated<T, P> {
    pub const fn new() -> Self {
        Self {
            inner: Vec::new(),
            last: None,
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.inner.is_empty() && self.last.is_none()
    }

    pub fn len(&self) -> usize {
        self.inner.len() + usize::from(self.last.is_some())
    }

    pub fn first(&self) -> Option<&T> {
        self.iter().next()
    }

    pub fn first_mut(&mut self) -> Option<&mut T> {
        self.iter_mut().next()
    }

    pub fn last(&self) -> Option<&T> {
        self.iter().next_back()
    }

    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.iter_mut().next_back()
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        if let Some((value, _)) = self.inner.get(index) {
            return Some(value);
        }
        if index == self.inner.len() {
            return self.last.as_deref();
        }
        None
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + Clone {
        self.inner
            .iter()
            .map(|(value, _)| value)
            .chain(self.last.as_deref())
    }

    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut T> {
        self.inner
            .iter_mut()
            .map(|(value, _)| value)
            .chain(self.last.as_deref_mut())
    }

    pub fn pairs(&self) -> impl DoubleEndedIterator<Item = Pair<&T, &P>> {
        self.inner
            .iter()
            .map(|(value, punct)| Pair::Punctuated(value, punct))
            .chain(self.last.as_deref().map(Pair::End))
    }

    pub fn into_pairs(self) -> impl DoubleEndedIterator<Item = Pair<T, P>> {
        self.inner
            .into_iter()
            .map(|(value, punct)| Pair::Punctuated(value, punct))
            .chain(self.last.map(|value| Pair::End(*value)))
    }

    pub fn push_value(&mut self, value: T) {
        assert!(
            self.empty_or_trailing(),
            "push_value after a value without punctuation"
        );
        self.last = Some(Box::new(value));
    }

    pub fn push_punct(&mut self, punct: P) {
        let last = self
            .last
            .take()
            .expect("push_punct with no value before it");
        self.inner.push((*last, punct));
    }

    pub fn pop(&mut self) -> Option<Pair<T, P>> {
        if let Some(last) = self.last.take() {
            return Some(Pair::End(*last));
        }
        self.inner
            .pop()
            .map(|(value, punct)| Pair::Punctuated(value, punct))
    }

    pub fn pop_punct(&mut self) -> Option<P> {
        if self.last.is_some() {
            return None;
        }
        let (value, punct) = self.inner.pop()?;
        self.last = Some(Box::new(value));
        Some(punct)
    }

    pub const fn trailing_punct(&self) -> bool {
        self.last.is_none() && !self.is_empty()
    }

    pub const fn empty_or_trailing(&self) -> bool {
        self.last.is_none()
    }

    pub fn clear(&mut self) {
        self.inner.clear();
        self.last = None;
    }

    pub fn push(&mut self, value: T)
    where
        P: Default,
    {
        if !self.empty_or_trailing() {
            self.push_punct(P::default());
        }
        self.push_value(value);
    }

    pub fn insert(&mut self, index: usize, value: T)
    where
        P: Default,
    {
        assert!(
            index <= self.len(),
            "Punctuated::insert: index out of range"
        );
        if index == self.len() {
            self.push(value);
        } else {
            self.inner.insert(index, (value, P::default()));
        }
    }
}

impl<T: crate::buffer::Parse, P: Token> Punctuated<T, P> {
    pub fn parse_terminated(input: ParseStream<'_>) -> Result<Self> {
        crate::buffer::parse_terminated(input, T::parse)
    }

    pub fn parse_separated_nonempty(input: ParseStream<'_>) -> Result<Self> {
        let mut punctuated = Self::new();
        loop {
            punctuated.push_value(T::parse(input)?);
            if !P::peek(input) {
                break;
            }
            punctuated.push_punct(P::parse_token(input)?);
        }
        Ok(punctuated)
    }
}

impl<T, P: Default> Extend<Pair<T, P>> for Punctuated<T, P> {
    fn extend<I: IntoIterator<Item = Pair<T, P>>>(&mut self, iter: I) {
        if !self.empty_or_trailing() {
            self.push_punct(P::default());
        }
        let mut ended = false;
        for pair in iter {
            assert!(!ended, "Punctuated extended with items after a Pair::End");
            match pair {
                Pair::Punctuated(value, punct) => self.inner.push((value, punct)),
                Pair::End(value) => {
                    self.last = Some(Box::new(value));
                    ended = true;
                }
            }
        }
    }
}

impl<T, P> Index<usize> for Punctuated<T, P> {
    type Output = T;

    fn index(&self, index: usize) -> &T {
        self.get(index).expect("Punctuated index out of range")
    }
}

impl<T, P> IndexMut<usize> for Punctuated<T, P> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        let length = self.inner.len();
        if index == length && self.last.is_some() {
            return self.last.as_deref_mut().expect("checked above");
        }
        &mut self.inner[index].0
    }
}

impl<T, P: Default> FromIterator<T> for Punctuated<T, P> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut punctuated = Self::new();
        punctuated.extend(iter);
        punctuated
    }
}

impl<T, P: Default> Extend<T> for Punctuated<T, P> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for value in iter {
            self.push(value);
        }
    }
}

impl<T, P> FromIterator<Pair<T, P>> for Punctuated<T, P> {
    fn from_iter<I: IntoIterator<Item = Pair<T, P>>>(iter: I) -> Self {
        let mut punctuated = Self::new();
        for pair in iter {
            match pair {
                Pair::Punctuated(value, punct) => {
                    punctuated.push_value(value);
                    punctuated.push_punct(punct);
                }
                Pair::End(value) => punctuated.push_value(value),
            }
        }
        punctuated
    }
}

impl<T, P> IntoIterator for Punctuated<T, P> {
    type Item = T;
    type IntoIter = std::iter::Chain<
        std::iter::Map<std::vec::IntoIter<(T, P)>, fn((T, P)) -> T>,
        std::iter::Map<std::option::IntoIter<Box<T>>, fn(Box<T>) -> T>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        let first: fn((T, P)) -> T = |(value, _)| value;
        let last: fn(Box<T>) -> T = |value| *value;
        self.inner
            .into_iter()
            .map(first)
            .chain(self.last.into_iter().map(last))
    }
}

impl<'a, T, P> IntoIterator for &'a Punctuated<T, P> {
    type Item = &'a T;
    type IntoIter = std::iter::Chain<
        std::iter::Map<std::slice::Iter<'a, (T, P)>, fn(&'a (T, P)) -> &'a T>,
        std::option::IntoIter<&'a T>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        let first: fn(&'a (T, P)) -> &'a T = |(value, _)| value;
        self.inner.iter().map(first).chain(self.last.as_deref())
    }
}

impl<'a, T, P> IntoIterator for &'a mut Punctuated<T, P> {
    type Item = &'a mut T;
    type IntoIter = std::iter::Chain<
        std::iter::Map<std::slice::IterMut<'a, (T, P)>, fn(&'a mut (T, P)) -> &'a mut T>,
        std::option::IntoIter<&'a mut T>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        let first: fn(&'a mut (T, P)) -> &'a mut T = |(value, _)| value;
        self.inner
            .iter_mut()
            .map(first)
            .chain(self.last.as_deref_mut())
    }
}

impl<T: ToTokens, P: ToTokens> ToTokens for Punctuated<T, P> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        for (value, punct) in &self.inner {
            value.to_tokens(tokens);
            punct.to_tokens(tokens);
        }
        if let Some(last) = &self.last {
            last.to_tokens(tokens);
        }
    }
}
