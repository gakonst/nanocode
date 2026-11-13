#![no_std]
#![forbid(unsafe_code)]

use core::fmt;
use core::ops::{BitAnd, Not};

pub trait Bits: Copy + Eq + BitAnd<Output = Self> + Not<Output = Self> + fmt::LowerHex {
    const EMPTY: Self;
}

macro_rules! bits {
    ($($bits:ty),*) => {
        $(impl Bits for $bits {
            const EMPTY: Self = 0;
        })*
    };
}

bits!(u8, u16, u32, i32);

pub struct IterNames<B: 'static> {
    named: &'static [(&'static str, B)],
    index: usize,
    source: B,
    remaining: B,
}

impl<B: Bits> IterNames<B> {
    pub const fn new(named: &'static [(&'static str, B)], source: B) -> Self {
        Self {
            named,
            index: 0,
            source,
            remaining: source,
        }
    }
}

impl<B: Bits> Iterator for IterNames<B> {
    type Item = (&'static str, B);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(&(name, bits)) = self.named.get(self.index) {
            if self.remaining == B::EMPTY {
                return None;
            }
            self.index += 1;
            if self.source & bits == bits && self.remaining & bits != B::EMPTY {
                self.remaining = self.remaining & !bits;
                return Some((name, bits));
            }
        }
        None
    }
}

pub struct Iter<B: 'static> {
    names: IterNames<B>,
    done: bool,
}

impl<B: Bits> Iter<B> {
    pub const fn new(named: &'static [(&'static str, B)], source: B) -> Self {
        Self {
            names: IterNames::new(named, source),
            done: false,
        }
    }
}

impl<B: Bits> Iterator for Iter<B> {
    type Item = B;

    fn next(&mut self) -> Option<B> {
        if let Some((_, bits)) = self.names.next() {
            return Some(bits);
        }
        if self.done {
            return None;
        }
        self.done = true;
        if self.names.remaining == B::EMPTY {
            None
        } else {
            Some(self.names.remaining)
        }
    }
}

pub struct Names<B: 'static> {
    named: &'static [(&'static str, B)],
    bits: B,
}

impl<B: Bits> Names<B> {
    pub const fn new(named: &'static [(&'static str, B)], bits: B) -> Self {
        Self { named, bits }
    }
}

impl<B: Bits> fmt::Debug for Names<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bits == B::EMPTY {
            return write!(formatter, "{:#x}", B::EMPTY);
        }
        let mut names = IterNames::new(self.named, self.bits);
        let mut first = true;
        for (name, _) in &mut names {
            if !first {
                formatter.write_str(" | ")?;
            }
            first = false;
            formatter.write_str(name)?;
        }
        if names.remaining != B::EMPTY {
            if !first {
                formatter.write_str(" | ")?;
            }
            write!(formatter, "{:#x}", names.remaining)?;
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! flags {
    (
        $(#[$outer:meta])*
        $visibility:vis struct $name:ident: $bits:ty {
            $(
                $(#[$inner:meta])*
                const $flag:ident = $value:expr;
            )*
        }
    ) => {
        $(#[$outer])*
        $visibility struct $name($bits);

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter
                    .debug_tuple(::core::stringify!($name))
                    .field(&$crate::Names::new(Self::NAMED_FLAGS, self.0))
                    .finish()
            }
        }

        $crate::flags! {
            impl $name: $bits {
                $(
                    $(#[$inner])*
                    const $flag = $value;
                )*
            }
        }
    };
    (
        impl $name:ident: $bits:ty {
            $(
                $(#[$inner:meta])*
                const $flag:ident = $value:expr;
            )*
        }
    ) => {
        impl $name {
            $(
                $(#[$inner])*
                pub const $flag: Self = Self($value);
            )*

            const NAMED_FLAGS: &'static [(&'static str, $bits)] =
                &[$((::core::stringify!($flag), Self::$flag.0)),*];

            pub const fn empty() -> Self {
                Self(0)
            }

            pub const fn all() -> Self {
                Self(0 $(| Self::$flag.0)*)
            }

            pub const fn bits(&self) -> $bits {
                self.0
            }

            pub const fn from_bits_truncate(bits: $bits) -> Self {
                Self(bits & Self::all().0)
            }

            pub const fn from_bits_retain(bits: $bits) -> Self {
                Self(bits)
            }

            pub const fn is_empty(&self) -> bool {
                self.0 == 0
            }

            pub const fn is_all(&self) -> bool {
                Self::all().0 | self.0 == self.0
            }

            pub const fn intersects(&self, other: Self) -> bool {
                self.0 & other.0 != 0
            }

            pub const fn contains(&self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            pub fn insert(&mut self, other: Self) {
                self.0 |= other.0;
            }

            pub fn remove(&mut self, other: Self) {
                self.0 &= !other.0;
            }

            pub fn set(&mut self, other: Self, value: bool) {
                if value {
                    self.0 |= other.0;
                } else {
                    self.0 &= !other.0;
                }
            }

            #[must_use]
            pub const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }

            #[must_use]
            pub const fn difference(self, other: Self) -> Self {
                Self(self.0 & !other.0)
            }

            pub fn iter(&self) -> impl ::core::iter::Iterator<Item = Self> + use<> {
                $crate::Iter::new(Self::NAMED_FLAGS, self.0).map(Self)
            }

            pub fn iter_names(&self) -> impl ::core::iter::Iterator<Item = (&'static str, Self)> + use<> {
                $crate::IterNames::new(Self::NAMED_FLAGS, self.0).map(|(name, bits)| (name, Self(bits)))
            }
        }

        impl ::core::ops::BitOr for $name {
            type Output = Self;

            fn bitor(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }
        }

        impl ::core::ops::BitOrAssign for $name {
            fn bitor_assign(&mut self, other: Self) {
                self.0 |= other.0;
            }
        }

        impl ::core::ops::BitAnd for $name {
            type Output = Self;

            fn bitand(self, other: Self) -> Self {
                Self(self.0 & other.0)
            }
        }

        impl ::core::ops::BitAndAssign for $name {
            fn bitand_assign(&mut self, other: Self) {
                self.0 &= other.0;
            }
        }

        impl ::core::ops::Sub for $name {
            type Output = Self;

            fn sub(self, other: Self) -> Self {
                Self(self.0 & !other.0)
            }
        }

        impl ::core::ops::SubAssign for $name {
            fn sub_assign(&mut self, other: Self) {
                self.0 &= !other.0;
            }
        }

        impl ::core::ops::Not for $name {
            type Output = Self;

            fn not(self) -> Self {
                Self(!self.0 & Self::all().0)
            }
        }
    };
}
