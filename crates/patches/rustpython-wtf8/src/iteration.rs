use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Display, Write};
use core::iter::{FusedIterator, Peekable};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L: Iterator, R: Iterator<Item = L::Item>> Iterator for Either<L, R> {
    type Item = L::Item;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Left(left) => left.next(),
            Self::Right(right) => right.next(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Left(left) => left.size_hint(),
            Self::Right(right) => right.size_hint(),
        }
    }
}

impl<L: DoubleEndedIterator, R: DoubleEndedIterator<Item = L::Item>> DoubleEndedIterator
    for Either<L, R>
{
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        match self {
            Self::Left(left) => left.next_back(),
            Self::Right(right) => right.next_back(),
        }
    }
}

impl<L: ExactSizeIterator, R: ExactSizeIterator<Item = L::Item>> ExactSizeIterator
    for Either<L, R>
{
}

impl<L: FusedIterator, R: FusedIterator<Item = L::Item>> FusedIterator for Either<L, R> {}

pub struct ExactlyOneError<I: Iterator> {
    first: Option<I::Item>,
    second: Option<I::Item>,
    rest: I,
}

impl<I: Iterator> Iterator for ExactlyOneError<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        self.first
            .take()
            .or_else(|| self.second.take())
            .or_else(|| self.rest.next())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let held = usize::from(self.first.is_some()) + usize::from(self.second.is_some());
        let (low, high) = self.rest.size_hint();
        (low + held, high.map(|high| high + held))
    }
}

impl<I: ExactSizeIterator> ExactSizeIterator for ExactlyOneError<I> {}

impl<I: Iterator<Item: fmt::Debug> + fmt::Debug> fmt::Debug for ExactlyOneError<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExactlyOneError")
            .field("first", &self.first)
            .field("second", &self.second)
            .field("rest", &self.rest)
            .finish()
    }
}

pub trait PeekingNext: Iterator {
    fn peeking_next<F>(&mut self, accept: F) -> Option<Self::Item>
    where
        F: FnOnce(&Self::Item) -> bool;
}

impl<I: Iterator> PeekingNext for Peekable<I> {
    fn peeking_next<F>(&mut self, accept: F) -> Option<Self::Item>
    where
        F: FnOnce(&Self::Item) -> bool,
    {
        self.next_if(accept)
    }
}

impl<T: PeekingNext + ?Sized> PeekingNext for &mut T {
    fn peeking_next<F>(&mut self, accept: F) -> Option<Self::Item>
    where
        F: FnOnce(&Self::Item) -> bool,
    {
        (**self).peeking_next(accept)
    }
}

pub struct PeekingTakeWhile<'a, I: ?Sized, F> {
    iterator: &'a mut I,
    predicate: F,
}

impl<I: PeekingNext + ?Sized, F: FnMut(&I::Item) -> bool> Iterator for PeekingTakeWhile<'_, I, F> {
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        self.iterator.peeking_next(&mut self.predicate)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.iterator.size_hint().1)
    }
}

pub struct Format<'a, I> {
    separator: &'a str,
    iterator: core::cell::Cell<Option<I>>,
}

impl<I: Iterator<Item: Display>> Display for Format<'_, I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut iterator = self
            .iterator
            .take()
            .expect("a Format is displayed at most once");
        if let Some(first) = iterator.next() {
            first.fmt(formatter)?;
            for item in iterator {
                formatter.write_str(self.separator)?;
                item.fmt(formatter)?;
            }
        }
        Ok(())
    }
}

pub struct ZipEq<A, B> {
    first: A,
    second: B,
}

impl<A: Iterator, B: Iterator> Iterator for ZipEq<A, B> {
    type Item = (A::Item, B::Item);

    fn next(&mut self) -> Option<Self::Item> {
        match (self.first.next(), self.second.next()) {
            (None, None) => None,
            (Some(first), Some(second)) => Some((first, second)),
            (None, Some(_)) | (Some(_), None) => {
                panic!("itertools: .zip_eq() reached end of one iterator before the other")
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (first_low, first_high) = self.first.size_hint();
        let (second_low, second_high) = self.second.size_hint();
        let high = match (first_high, second_high) {
            (Some(first), Some(second)) => Some(first.min(second)),
            (high, None) | (None, high) => high,
        };
        (first_low.min(second_low), high)
    }
}

impl<A: ExactSizeIterator, B: ExactSizeIterator> ExactSizeIterator for ZipEq<A, B> {}

pub struct Pairs<I> {
    iterator: I,
}

impl<I: Iterator> Iterator for Pairs<I> {
    type Item = (I::Item, I::Item);

    fn next(&mut self) -> Option<Self::Item> {
        let first = self.iterator.next()?;
        let second = self.iterator.next()?;
        Some((first, second))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (low, high) = self.iterator.size_hint();
        (low / 2, high.map(|high| high / 2))
    }
}

pub struct Intersperse<I: Iterator> {
    iterator: Peekable<I>,
    separator: I::Item,
    separator_next: bool,
}

impl<I: Iterator<Item: Clone>> Iterator for Intersperse<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        if self.separator_next {
            self.iterator.peek()?;
            self.separator_next = false;
            return Some(self.separator.clone());
        }
        let item = self.iterator.next()?;
        self.separator_next = true;
        Some(item)
    }
}

pub struct ProcessResults<'a, I, E> {
    iterator: I,
    error: &'a mut Option<E>,
}

impl<I: Iterator<Item = Result<T, E>>, T, E> Iterator for ProcessResults<'_, I, E> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        match self.iterator.next() {
            Some(Ok(value)) => Some(value),
            Some(Err(error)) => {
                *self.error = Some(error);
                None
            }
            None => None,
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.iterator.size_hint().1)
    }
}

pub fn process_results<I, F, T, E, R>(iterable: I, processor: F) -> Result<R, E>
where
    I: IntoIterator<Item = Result<T, E>>,
    F: FnOnce(ProcessResults<'_, I::IntoIter, E>) -> R,
{
    let mut error = None;
    let result = processor(ProcessResults {
        iterator: iterable.into_iter(),
        error: &mut error,
    });
    match error {
        Some(error) => Err(error),
        None => Ok(result),
    }
}

pub trait Itertools: Iterator {
    fn collect_vec(self) -> Vec<Self::Item>
    where
        Self: Sized,
    {
        self.collect()
    }

    fn exactly_one(mut self) -> Result<Self::Item, ExactlyOneError<Self>>
    where
        Self: Sized,
    {
        match self.next() {
            None => Err(ExactlyOneError {
                first: None,
                second: None,
                rest: self,
            }),
            Some(first) => match self.next() {
                None => Ok(first),
                Some(second) => Err(ExactlyOneError {
                    first: Some(first),
                    second: Some(second),
                    rest: self,
                }),
            },
        }
    }

    fn contains<Q>(&mut self, query: &Q) -> bool
    where
        Self::Item: core::borrow::Borrow<Q>,
        Q: PartialEq + ?Sized,
        Self: Sized,
    {
        self.any(|item| core::borrow::Borrow::borrow(&item) == query)
    }

    fn join(&mut self, separator: &str) -> String
    where
        Self::Item: Display,
    {
        let mut text = String::new();
        if let Some(first) = self.next() {
            write!(text, "{first}").expect("writing to a String does not fail");
            for item in self {
                text.push_str(separator);
                write!(text, "{item}").expect("writing to a String does not fail");
            }
        }
        text
    }

    fn format(self, separator: &str) -> Format<'_, Self>
    where
        Self: Sized,
    {
        Format {
            separator,
            iterator: core::cell::Cell::new(Some(self)),
        }
    }

    fn map_into<T>(self) -> core::iter::Map<Self, fn(Self::Item) -> T>
    where
        Self: Sized,
        Self::Item: Into<T>,
    {
        self.map(Into::into)
    }

    fn peeking_take_while<F>(&mut self, predicate: F) -> PeekingTakeWhile<'_, Self, F>
    where
        Self: PeekingNext,
        F: FnMut(&Self::Item) -> bool,
    {
        PeekingTakeWhile {
            iterator: self,
            predicate,
        }
    }

    fn try_collect<T, U, E>(self) -> Result<U, E>
    where
        Self: Sized + Iterator<Item = Result<T, E>>,
        Result<U, E>: FromIterator<Result<T, E>>,
    {
        self.collect()
    }

    fn pairs(self) -> Pairs<Self>
    where
        Self: Sized,
    {
        Pairs { iterator: self }
    }

    fn zip_eq<J: IntoIterator>(self, other: J) -> ZipEq<Self, J::IntoIter>
    where
        Self: Sized,
    {
        ZipEq {
            first: self,
            second: other.into_iter(),
        }
    }

    fn intersperse(self, separator: Self::Item) -> Intersperse<Self>
    where
        Self: Sized,
        Self::Item: Clone,
    {
        Intersperse {
            iterator: self.peekable(),
            separator,
            separator_next: false,
        }
    }

    fn collect_array<const LENGTH: usize>(mut self) -> Option<[Self::Item; LENGTH]>
    where
        Self: Sized,
    {
        let items: Vec<Self::Item> = self.by_ref().take(LENGTH).collect();
        if self.next().is_some() {
            return None;
        }
        items.try_into().ok()
    }

    fn process_results<F, T, E, R>(self, processor: F) -> Result<R, E>
    where
        Self: Sized + Iterator<Item = Result<T, E>>,
        F: FnOnce(ProcessResults<'_, Self, E>) -> R,
    {
        process_results(self, processor)
    }
}

impl<I: Iterator + ?Sized> Itertools for I {}
