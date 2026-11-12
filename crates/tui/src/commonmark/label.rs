use std::hash::{Hash, Hasher};

use super::case_fold::case_fold;
use super::scan::{is_ascii_punctuation, is_ascii_whitespace, scan_end_of_line};
use super::text::CowStr;

const LABEL_END_BYTE: u8 = 0xFF;
const LINK_LABEL_CODE_POINTS_MAX: usize = 1000;

#[derive(Debug)]
pub(super) enum ReferenceLabel<'a> {
    Link(CowStr<'a>),
    Footnote(CowStr<'a>),
}

#[derive(Clone, Debug)]
pub(super) struct LinkLabel<'a>(CowStr<'a>);

enum Folded {
    Mapped(std::str::Chars<'static>),
    Same(Option<char>),
}

impl Iterator for Folded {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self {
            Self::Mapped(characters) => characters.next(),
            Self::Same(character) => character.take(),
        }
    }
}

fn folded(character: char) -> Folded {
    match case_fold(character) {
        Some(mapped) => Folded::Mapped(mapped.chars()),
        None => Folded::Same(Some(character)),
    }
}

impl<'a> LinkLabel<'a> {
    pub(super) const fn new(label: CowStr<'a>) -> Self {
        Self(label)
    }

    fn folded(&self) -> impl Iterator<Item = char> + '_ {
        self.0.chars().flat_map(folded)
    }
}

impl PartialEq for LinkLabel<'_> {
    fn eq(&self, other: &Self) -> bool {
        if self.0.is_ascii() && other.0.is_ascii() {
            return self.0.eq_ignore_ascii_case(&other.0);
        }
        self.folded().eq(other.folded())
    }
}

impl Eq for LinkLabel<'_> {}

impl Hash for LinkLabel<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if self.0.is_ascii() {
            self.0
                .bytes()
                .for_each(|byte| state.write_u8(byte.to_ascii_lowercase()));
        } else {
            let mut buffer = [0; 4];
            for character in self.folded() {
                character
                    .encode_utf8(&mut buffer)
                    .bytes()
                    .for_each(|byte| state.write_u8(byte));
            }
        }
        state.write_u8(LABEL_END_BYTE);
    }
}

pub(crate) fn scan_link_label_rest<'t>(
    text: &'t str,
    line_break_handler: &dyn Fn(&[u8]) -> Option<usize>,
    is_in_table: bool,
) -> Option<(usize, CowStr<'t>)> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut only_whitespace = true;
    let mut code_points = 0;

    let mut label = String::new();
    let mut mark = 0;

    loop {
        if code_points >= LINK_LABEL_CODE_POINTS_MAX {
            return None;
        }
        match *bytes.get(index)? {
            b'[' => return None,
            b']' => break,

            b'|' if is_in_table && index != 0 && bytes.get(index - 1) == Some(&b'\\') => {
                label.push_str(&text[mark..index - 1]);
                label.push('|');
                index += 1;
                only_whitespace = false;
                mark = index;
            }
            b'\\' if is_in_table && bytes.get(index + 1) == Some(&b'|') => {
                label.push_str(&text[mark..index]);
                label.push('|');
                index += 2;
                code_points += 1;
                only_whitespace = false;
                mark = index;
            }
            b'\\' if is_ascii_punctuation(*bytes.get(index + 1)?) => {
                index += 2;
                code_points += 2;
                only_whitespace = false;
            }
            byte if is_ascii_whitespace(byte) => {
                let mut whitespaces = 0;
                let mut line_breaks = 0;
                let whitespace_start = index;

                while index < bytes.len() && is_ascii_whitespace(bytes[index]) {
                    if let Some(end_of_line_bytes) = scan_end_of_line(&bytes[index..]) {
                        line_breaks += 1;
                        if line_breaks > 1 {
                            return None;
                        }
                        index += end_of_line_bytes;
                        index += line_break_handler(&bytes[index..])?;
                        whitespaces += 2;
                    } else {
                        whitespaces += if bytes[index] == b' ' { 1 } else { 2 };
                        index += 1;
                    }
                }
                if whitespaces > 1 {
                    label.push_str(&text[mark..whitespace_start]);
                    label.push(' ');
                    mark = index;
                    code_points += index - whitespace_start;
                } else {
                    code_points += 1;
                }
            }
            byte => {
                only_whitespace = false;
                index += 1;
                if !byte.is_ascii() {
                    code_points += 1;
                }
            }
        }
    }

    if only_whitespace {
        None
    } else {
        let string = if mark == 0 {
            let ascii_whitespace = &[' ', '\r', '\n', '\t'][..];
            text[..index].trim_matches(ascii_whitespace).into()
        } else {
            label.push_str(&text[mark..index]);
            while matches!(
                label.as_bytes().last(),
                Some(&b' ' | &b'\r' | &b'\n' | &b'\t')
            ) {
                label.pop();
            }
            while matches!(
                label.as_bytes().first(),
                Some(&b' ' | &b'\r' | &b'\n' | &b'\t')
            ) {
                label.remove(0);
            }
            label.into()
        };
        Some((index + 1, string))
    }
}
