use std::str::Chars;

mod compose;
mod decompose;
mod lookup;
#[rustfmt::skip]
mod tables;

pub use crate::compose::Recompositions;
use crate::lookup::Form;

pub trait UnicodeNormalization<I: Iterator<Item = char>> {
    fn nfc(self) -> Recompositions<I>;

    fn nfkc(self) -> Recompositions<I>;
}

impl<'a> UnicodeNormalization<Chars<'a>> for &'a str {
    fn nfc(self) -> Recompositions<Chars<'a>> {
        Recompositions::new(self.chars(), Form::Canonical)
    }

    fn nfkc(self) -> Recompositions<Chars<'a>> {
        Recompositions::new(self.chars(), Form::Compatibility)
    }
}

impl<I: Iterator<Item = char>> UnicodeNormalization<I> for I {
    fn nfc(self) -> Recompositions<Self> {
        Recompositions::new(self, Form::Canonical)
    }

    fn nfkc(self) -> Recompositions<Self> {
        Recompositions::new(self, Form::Compatibility)
    }
}
