macro_rules! option_like {
    ($name:ident, $primary:ident, $secondary:ident) => {
        impl<T> $name<T> {
            #[inline]
            pub fn from_option(option: Option<T>) -> Self {
                match option {
                    Some(value) => Self::$primary(value),
                    None => Self::$secondary,
                }
            }

            #[inline]
            pub fn into_option(self) -> Option<T> {
                match self {
                    Self::$primary(value) => Some(value),
                    Self::$secondary => None,
                }
            }

            #[inline]
            pub const fn as_option(&self) -> Option<&T> {
                match self {
                    Self::$primary(value) => Some(value),
                    Self::$secondary => None,
                }
            }

            #[inline]
            pub const fn as_option_mut(&mut self) -> Option<&mut T> {
                match self {
                    Self::$primary(value) => Some(value),
                    Self::$secondary => None,
                }
            }

            #[inline]
            pub fn expect(self, message: &str) -> T {
                self.into_option().expect(message)
            }

            #[inline]
            pub fn unwrap(self) -> T {
                self.into_option().unwrap()
            }

            #[inline]
            pub fn unwrap_or(self, default: T) -> T {
                self.into_option().unwrap_or(default)
            }

            #[inline]
            pub fn unwrap_or_else<F: FnOnce() -> T>(self, function: F) -> T {
                self.into_option().unwrap_or_else(function)
            }

            #[inline]
            pub fn ok_or<E>(self, error: E) -> Result<T, E> {
                self.into_option().ok_or(error)
            }

            #[inline]
            pub fn ok_or_else<E, F: FnOnce() -> E>(self, error: F) -> Result<T, E> {
                self.into_option().ok_or_else(error)
            }

            #[inline]
            pub fn filter<P: FnOnce(&T) -> bool>(self, predicate: P) -> Self {
                Self::from_option(self.into_option().filter(predicate))
            }

            #[inline]
            pub fn or(self, other: Self) -> Self {
                Self::from_option(self.into_option().or(other.into_option()))
            }

            #[inline]
            pub fn or_else<F: FnOnce() -> Self>(self, function: F) -> Self {
                Self::from_option(self.into_option().or_else(|| function().into_option()))
            }

            #[inline]
            pub fn map_or<U, F: FnOnce(T) -> U>(self, default: U, function: F) -> U {
                self.into_option().map_or(default, function)
            }

            #[inline]
            pub fn xor(self, other: Self) -> Self {
                Self::from_option(self.into_option().xor(other.into_option()))
            }

            #[inline]
            pub fn get_or_insert(&mut self, value: T) -> &mut T {
                self.get_or_insert_with(|| value)
            }

            #[inline]
            pub fn get_or_insert_with<F: FnOnce() -> T>(&mut self, function: F) -> &mut T {
                if let Self::$secondary = self {
                    *self = Self::$primary(function());
                }
                match self {
                    Self::$primary(value) => value,
                    Self::$secondary => unreachable!(),
                }
            }

            #[inline]
            pub fn replace(&mut self, value: T) -> Self {
                core::mem::replace(self, Self::$primary(value))
            }

            #[inline]
            pub const fn as_ref(&self) -> $name<&T> {
                match self {
                    Self::$primary(value) => $name::$primary(value),
                    Self::$secondary => $name::$secondary,
                }
            }

            #[inline]
            pub const fn as_mut(&mut self) -> $name<&mut T> {
                match self {
                    Self::$primary(value) => $name::$primary(value),
                    Self::$secondary => $name::$secondary,
                }
            }

            #[inline]
            pub fn map<U, F: FnOnce(T) -> U>(self, function: F) -> $name<U> {
                match self {
                    Self::$primary(value) => $name::$primary(function(value)),
                    Self::$secondary => $name::$secondary,
                }
            }

            #[inline]
            pub fn map_or_else<U, D: FnOnce() -> U, F: FnOnce(T) -> U>(self, default: D, function: F) -> U {
                self.into_option().map_or_else(default, function)
            }

            pub fn iter(&self) -> impl core::iter::Iterator<Item = &T> {
                self.as_option().into_iter()
            }

            pub fn iter_mut(&mut self) -> impl core::iter::Iterator<Item = &mut T> {
                self.as_option_mut().into_iter()
            }

            #[inline]
            pub fn and<U>(self, other: $name<U>) -> $name<U> {
                match self {
                    Self::$primary(_) => other,
                    Self::$secondary => $name::$secondary,
                }
            }

            #[inline]
            pub fn and_then<U, F: FnOnce(T) -> $name<U>>(self, function: F) -> $name<U> {
                match self {
                    Self::$primary(value) => function(value),
                    Self::$secondary => $name::$secondary,
                }
            }

            pub fn zip<U>(self, other: $name<U>) -> $name<(T, U)> {
                $name::from_option(self.into_option().zip(other.into_option()))
            }

            #[inline]
            pub fn take(&mut self) -> Self {
                core::mem::replace(self, Self::$secondary)
            }

            #[inline]
            pub fn unwrap_or_default(self) -> T
            where
                T: Default,
            {
                self.into_option().unwrap_or_default()
            }
        }

        impl<T> From<Option<T>> for $name<T> {
            #[inline]
            fn from(value: Option<T>) -> Self {
                Self::from_option(value)
            }
        }

        impl<T> From<$name<T>> for Option<T> {
            #[inline]
            fn from(value: $name<T>) -> Self {
                value.into_option()
            }
        }

        impl<T> From<T> for $name<T> {
            #[inline]
            fn from(value: T) -> Self {
                Self::$primary(value)
            }
        }

        impl<T> Default for $name<T> {
            #[inline]
            fn default() -> Self {
                Self::$secondary
            }
        }

        impl<T> IntoIterator for $name<T> {
            type Item = T;
            type IntoIter = core::option::IntoIter<T>;

            #[inline]
            fn into_iter(self) -> core::option::IntoIter<T> {
                self.into_option().into_iter()
            }
        }

        impl<T: Copy> Copy for $name<T> {}

        impl<T: Clone> Clone for $name<T> {
            #[inline]
            fn clone(&self) -> Self {
                match self {
                    Self::$primary(value) => Self::$primary(value.clone()),
                    Self::$secondary => Self::$secondary,
                }
            }

            #[inline]
            fn clone_from(&mut self, source: &Self) {
                match (self, source) {
                    (Self::$primary(to), Self::$primary(from)) => to.clone_from(from),
                    (to, from) => *to = from.clone(),
                }
            }
        }

        impl<T: Copy> $name<&T> {
            pub fn copied(self) -> $name<T> {
                self.map(|&value| value)
            }
        }

        impl<T: Clone> $name<&T> {
            pub fn cloned(self) -> $name<T> {
                self.map(Clone::clone)
            }
        }

        impl<T: core::ops::Deref> $name<T> {
            pub fn as_deref(&self) -> $name<&T::Target> {
                self.as_ref().map(core::ops::Deref::deref)
            }
        }

        impl<T: core::ops::DerefMut> $name<T> {
            pub fn as_deref_mut(&mut self) -> $name<&mut T::Target> {
                self.as_mut().map(core::ops::DerefMut::deref_mut)
            }
        }

        impl<T, E> $name<Result<T, E>> {
            #[inline]
            pub fn transpose(self) -> Result<$name<T>, E> {
                self.into_option().transpose().map($name::from_option)
            }
        }
    };
}

pub(crate) use option_like;
