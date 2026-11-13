use std::str::Chars;

mod compose;
mod decompose;
mod lookup;
#[rustfmt::skip]
mod tables;

pub use crate::compose::Recompositions;
pub use crate::decompose::Decompositions;
use crate::lookup::Form;
pub use crate::lookup::combining_class;

pub trait UnicodeNormalization<I: Iterator<Item = char>> {
    fn nfd(self) -> Decompositions<I>;

    fn nfkd(self) -> Decompositions<I>;

    fn nfc(self) -> Recompositions<I>;

    fn nfkc(self) -> Recompositions<I>;
}

impl<'a> UnicodeNormalization<Chars<'a>> for &'a str {
    fn nfd(self) -> Decompositions<Chars<'a>> {
        Decompositions::new(self.chars(), Form::Canonical)
    }

    fn nfkd(self) -> Decompositions<Chars<'a>> {
        Decompositions::new(self.chars(), Form::Compatibility)
    }

    fn nfc(self) -> Recompositions<Chars<'a>> {
        Recompositions::new(self.chars(), Form::Canonical)
    }

    fn nfkc(self) -> Recompositions<Chars<'a>> {
        Recompositions::new(self.chars(), Form::Compatibility)
    }
}

impl<I: Iterator<Item = char>> UnicodeNormalization<I> for I {
    fn nfd(self) -> Decompositions<Self> {
        Decompositions::new(self, Form::Canonical)
    }

    fn nfkd(self) -> Decompositions<Self> {
        Decompositions::new(self, Form::Compatibility)
    }

    fn nfc(self) -> Recompositions<Self> {
        Recompositions::new(self, Form::Canonical)
    }

    fn nfkc(self) -> Recompositions<Self> {
        Recompositions::new(self, Form::Compatibility)
    }
}
