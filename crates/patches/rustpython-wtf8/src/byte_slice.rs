use alloc::vec::Vec;

use memchr::{memchr, memchr2, memchr3, memmem, memrchr, memrchr2, memrchr3};

const UTF8_SEQUENCE_BYTES_MAX: usize = 4;
const CONTINUATION_BYTE_MASK: u8 = 0b1100_0000;
const CONTINUATION_BYTE_TAG: u8 = 0b1000_0000;
const BYTE_VALUES: usize = 256;

pub trait ByteSlice {
    fn find<B: AsRef<[u8]>>(&self, needle: B) -> Option<usize>;
    fn rfind<B: AsRef<[u8]>>(&self, needle: B) -> Option<usize>;
    fn find_iter<'h, 'n, B: ?Sized + AsRef<[u8]>>(
        &'h self,
        needle: &'n B,
    ) -> memmem::FindIter<'h, 'n>;
    fn find_byte(&self, byte: u8) -> Option<usize>;
    fn find_byteset<B: AsRef<[u8]>>(&self, byteset: B) -> Option<usize>;
    fn rfind_byteset<B: AsRef<[u8]>>(&self, byteset: B) -> Option<usize>;
    fn contains_str<B: AsRef<[u8]>>(&self, needle: B) -> bool;
    fn starts_with_str<B: AsRef<[u8]>>(&self, prefix: B) -> bool;
    fn ends_with_str<B: AsRef<[u8]>>(&self, suffix: B) -> bool;
    fn split_str<'h, 's, B: ?Sized + AsRef<[u8]>>(&'h self, separator: &'s B) -> Split<'h, 's>;
    fn rsplit_str<'h, 's, B: ?Sized + AsRef<[u8]>>(
        &'h self,
        separator: &'s B,
    ) -> SplitReverse<'h, 's>;
    fn splitn_str<'h, 's, B: ?Sized + AsRef<[u8]>>(
        &'h self,
        limit: usize,
        separator: &'s B,
    ) -> SplitLimit<'h, 's>;
    fn rsplitn_str<'h, 's, B: ?Sized + AsRef<[u8]>>(
        &'h self,
        limit: usize,
        separator: &'s B,
    ) -> SplitLimitReverse<'h, 's>;
    fn replace<N: AsRef<[u8]>, R: AsRef<[u8]>>(&self, needle: N, replacement: R) -> Vec<u8>;
    fn replacen<N: AsRef<[u8]>, R: AsRef<[u8]>>(
        &self,
        needle: N,
        replacement: R,
        limit: usize,
    ) -> Vec<u8>;
    fn trim(&self) -> &[u8];
    fn trim_start(&self) -> &[u8];
    fn trim_end(&self) -> &[u8];
    fn trim_with<F: FnMut(char) -> bool>(&self, trim: F) -> &[u8];
    fn trim_start_with<F: FnMut(char) -> bool>(&self, trim: F) -> &[u8];
    fn trim_end_with<F: FnMut(char) -> bool>(&self, trim: F) -> &[u8];
}

impl ByteSlice for [u8] {
    #[inline]
    fn find<B: AsRef<[u8]>>(&self, needle: B) -> Option<usize> {
        memmem::find(self, needle.as_ref())
    }

    #[inline]
    fn rfind<B: AsRef<[u8]>>(&self, needle: B) -> Option<usize> {
        memmem::rfind(self, needle.as_ref())
    }

    #[inline]
    fn find_iter<'h, 'n, B: ?Sized + AsRef<[u8]>>(
        &'h self,
        needle: &'n B,
    ) -> memmem::FindIter<'h, 'n> {
        memmem::find_iter(self, needle.as_ref())
    }

    #[inline]
    fn find_byte(&self, byte: u8) -> Option<usize> {
        memchr(byte, self)
    }

    fn find_byteset<B: AsRef<[u8]>>(&self, byteset: B) -> Option<usize> {
        match *byteset.as_ref() {
            [] => None,
            [first] => memchr(first, self),
            [first, second] => memchr2(first, second, self),
            [first, second, third] => memchr3(first, second, third, self),
            ref set => {
                let table = byte_table(set);
                self.iter().position(|&byte| table[usize::from(byte)])
            }
        }
    }

    fn rfind_byteset<B: AsRef<[u8]>>(&self, byteset: B) -> Option<usize> {
        match *byteset.as_ref() {
            [] => None,
            [first] => memrchr(first, self),
            [first, second] => memrchr2(first, second, self),
            [first, second, third] => memrchr3(first, second, third, self),
            ref set => {
                let table = byte_table(set);
                self.iter().rposition(|&byte| table[usize::from(byte)])
            }
        }
    }

    #[inline]
    fn contains_str<B: AsRef<[u8]>>(&self, needle: B) -> bool {
        self.find(needle).is_some()
    }

    #[inline]
    fn starts_with_str<B: AsRef<[u8]>>(&self, prefix: B) -> bool {
        self.starts_with(prefix.as_ref())
    }

    #[inline]
    fn ends_with_str<B: AsRef<[u8]>>(&self, suffix: B) -> bool {
        self.ends_with(suffix.as_ref())
    }

    #[inline]
    fn split_str<'h, 's, B: ?Sized + AsRef<[u8]>>(&'h self, separator: &'s B) -> Split<'h, 's> {
        Split::new(self, separator.as_ref())
    }

    #[inline]
    fn rsplit_str<'h, 's, B: ?Sized + AsRef<[u8]>>(
        &'h self,
        separator: &'s B,
    ) -> SplitReverse<'h, 's> {
        SplitReverse::new(self, separator.as_ref())
    }

    #[inline]
    fn splitn_str<'h, 's, B: ?Sized + AsRef<[u8]>>(
        &'h self,
        limit: usize,
        separator: &'s B,
    ) -> SplitLimit<'h, 's> {
        SplitLimit {
            split: Split::new(self, separator.as_ref()),
            limit,
            count: 0,
        }
    }

    #[inline]
    fn rsplitn_str<'h, 's, B: ?Sized + AsRef<[u8]>>(
        &'h self,
        limit: usize,
        separator: &'s B,
    ) -> SplitLimitReverse<'h, 's> {
        SplitLimitReverse {
            split: SplitReverse::new(self, separator.as_ref()),
            limit,
            count: 0,
        }
    }

    fn replace<N: AsRef<[u8]>, R: AsRef<[u8]>>(&self, needle: N, replacement: R) -> Vec<u8> {
        replace_matches(self, needle.as_ref(), replacement.as_ref(), usize::MAX)
    }

    fn replacen<N: AsRef<[u8]>, R: AsRef<[u8]>>(
        &self,
        needle: N,
        replacement: R,
        limit: usize,
    ) -> Vec<u8> {
        replace_matches(self, needle.as_ref(), replacement.as_ref(), limit)
    }

    #[inline]
    fn trim(&self) -> &[u8] {
        self.trim_start().trim_end()
    }

    fn trim_start(&self) -> &[u8] {
        let mut rest = self;
        while let (Some(character), length) = decode_first(rest)
            && character.is_whitespace()
        {
            rest = &rest[length..];
        }
        rest
    }

    fn trim_end(&self) -> &[u8] {
        let mut rest = self;
        while let (Some(character), length) = decode_last(rest)
            && character.is_whitespace()
        {
            rest = &rest[..rest.len() - length];
        }
        rest
    }

    #[inline]
    fn trim_with<F: FnMut(char) -> bool>(&self, mut trim: F) -> &[u8] {
        self.trim_start_with(&mut trim).trim_end_with(&mut trim)
    }

    fn trim_start_with<F: FnMut(char) -> bool>(&self, mut trim: F) -> &[u8] {
        let mut rest = self;
        while !rest.is_empty() {
            let (character, length) = decode_first(rest);
            if !trim(character.unwrap_or(char::REPLACEMENT_CHARACTER)) {
                return rest;
            }
            rest = &rest[length..];
        }
        b""
    }

    fn trim_end_with<F: FnMut(char) -> bool>(&self, mut trim: F) -> &[u8] {
        let mut rest = self;
        while !rest.is_empty() {
            let (character, length) = decode_last(rest);
            if !trim(character.unwrap_or(char::REPLACEMENT_CHARACTER)) {
                return rest;
            }
            rest = &rest[..rest.len() - length];
        }
        b""
    }
}

fn byte_table(set: &[u8]) -> [bool; BYTE_VALUES] {
    let mut table = [false; BYTE_VALUES];
    for &byte in set {
        table[usize::from(byte)] = true;
    }
    table
}

fn replace_matches(haystack: &[u8], needle: &[u8], replacement: &[u8], limit: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(haystack.len());
    let mut last = 0;
    for start in memmem::find_iter(haystack, needle).take(limit) {
        result.extend_from_slice(&haystack[last..start]);
        result.extend_from_slice(replacement);
        last = start + needle.len();
    }
    result.extend_from_slice(&haystack[last..]);
    result
}

fn decode_first(bytes: &[u8]) -> (Option<char>, usize) {
    if let Some(&byte) = bytes.first()
        && byte.is_ascii()
    {
        return (Some(char::from(byte)), 1);
    }
    let window = &bytes[..bytes.len().min(UTF8_SEQUENCE_BYTES_MAX)];
    let Some(chunk) = window.utf8_chunks().next() else {
        return (None, 0);
    };
    match chunk.valid().chars().next() {
        Some(character) => (Some(character), character.len_utf8()),
        None => {
            assert!(!chunk.invalid().is_empty());
            (None, chunk.invalid().len())
        }
    }
}

fn decode_last(bytes: &[u8]) -> (Option<char>, usize) {
    let Some(&last) = bytes.last() else {
        return (None, 0);
    };
    if last.is_ascii() {
        return (Some(char::from(last)), 1);
    }
    let mut start = bytes.len() - 1;
    let limit = bytes.len().saturating_sub(UTF8_SEQUENCE_BYTES_MAX);
    while start > limit && bytes[start] & CONTINUATION_BYTE_MASK == CONTINUATION_BYTE_TAG {
        start -= 1;
    }
    let (character, length) = decode_first(&bytes[start..]);
    if start + length == bytes.len() {
        (character, length)
    } else {
        (None, 1)
    }
}

#[derive(Clone, Debug)]
pub struct Split<'h, 's> {
    haystack: &'h [u8],
    separator: &'s [u8],
    matches: memmem::FindIter<'h, 's>,
    last: usize,
    done: bool,
}

impl<'h, 's> Split<'h, 's> {
    fn new(haystack: &'h [u8], separator: &'s [u8]) -> Self {
        Self {
            haystack,
            separator,
            matches: memmem::find_iter(haystack, separator),
            last: 0,
            done: false,
        }
    }
}

impl<'h> Iterator for Split<'h, '_> {
    type Item = &'h [u8];

    fn next(&mut self) -> Option<&'h [u8]> {
        if let Some(start) = self.matches.next() {
            let piece = &self.haystack[self.last..start];
            self.last = start + self.separator.len();
            return Some(piece);
        }
        if self.done {
            return None;
        }
        self.done = true;
        let piece = &self.haystack[self.last..];
        self.last = self.haystack.len();
        Some(piece)
    }
}

#[derive(Clone, Debug)]
pub struct SplitReverse<'h, 's> {
    haystack: &'h [u8],
    separator: &'s [u8],
    matches: memmem::FindRevIter<'h, 's>,
    last: usize,
    done: bool,
}

impl<'h, 's> SplitReverse<'h, 's> {
    fn new(haystack: &'h [u8], separator: &'s [u8]) -> Self {
        Self {
            haystack,
            separator,
            matches: memmem::rfind_iter(haystack, separator),
            last: haystack.len(),
            done: false,
        }
    }
}

impl<'h> Iterator for SplitReverse<'h, '_> {
    type Item = &'h [u8];

    fn next(&mut self) -> Option<&'h [u8]> {
        if let Some(start) = self.matches.next() {
            let piece = &self.haystack[start + self.separator.len()..self.last];
            self.last = start;
            return Some(piece);
        }
        if self.done {
            return None;
        }
        self.done = true;
        let piece = &self.haystack[..self.last];
        self.last = 0;
        Some(piece)
    }
}

#[derive(Clone, Debug)]
pub struct SplitLimit<'h, 's> {
    split: Split<'h, 's>,
    limit: usize,
    count: usize,
}

impl<'h> Iterator for SplitLimit<'h, '_> {
    type Item = &'h [u8];

    fn next(&mut self) -> Option<&'h [u8]> {
        self.count += 1;
        if self.count > self.limit || self.split.done {
            None
        } else if self.count == self.limit {
            Some(&self.split.haystack[self.split.last..])
        } else {
            self.split.next()
        }
    }
}

#[derive(Clone, Debug)]
pub struct SplitLimitReverse<'h, 's> {
    split: SplitReverse<'h, 's>,
    limit: usize,
    count: usize,
}

impl<'h> Iterator for SplitLimitReverse<'h, '_> {
    type Item = &'h [u8];

    fn next(&mut self) -> Option<&'h [u8]> {
        self.count += 1;
        if self.count > self.limit || self.split.done {
            None
        } else if self.count == self.limit {
            Some(&self.split.haystack[..self.split.last])
        } else {
            self.split.next()
        }
    }
}
