use memchr::memchr;

use super::entities;
use super::parse::HtmlScanGuard;
pub(super) use super::punctuation::{is_ascii_punctuation, is_punctuation};
use super::text::CowStr;
use super::{Alignment, BlockQuoteKind, HeadingLevel, LinkType};

pub(super) const TAB_STOP_COLUMNS: usize = 4;
pub(super) const CODE_INDENT_COLUMNS: usize = 4;
pub(super) const BLOCK_INDENT_COLUMNS_MAX: usize = 3;
const THEMATIC_BREAK_MARKERS_MIN: usize = 3;
const FENCE_LENGTH_MIN: usize = 3;
const ORDERED_LIST_DIGITS_MAX: usize = 9;
const ORDERED_LIST_MARKER_BYTES_MAX: usize = ORDERED_LIST_DIGITS_MAX + 1;
const ENTITY_HEX_DIGITS_MAX: usize = 6;
const ENTITY_DECIMAL_DIGITS_MAX: usize = 7;
const DECIMAL_RADIX: u32 = 10;
const HEX_RADIX: u32 = 16;
const URI_SCHEME_LENGTH_MIN: usize = 2;
const URI_SCHEME_LENGTH_MAX: usize = 32;
const EMAIL_LABEL_LENGTH_MAX: usize = 63;

pub(super) type NewlineHandler<'a> = Option<&'a dyn Fn(&[u8]) -> usize>;

const HTML_TAGS: [&str; 62] = [
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

#[derive(Clone)]
pub(crate) struct LineStart<'a> {
    bytes: &'a [u8],
    index: usize,
    tab_start: usize,
    spaces_remaining: usize,
    min_thematic_break_offset: usize,
}

impl<'a> LineStart<'a> {
    pub(crate) const fn new(bytes: &[u8]) -> LineStart<'_> {
        LineStart {
            bytes,
            tab_start: 0,
            index: 0,
            spaces_remaining: 0,
            min_thematic_break_offset: 0,
        }
    }

    pub(crate) fn scan_space(&mut self, spaces_wanted: usize) -> bool {
        self.scan_space_inner(spaces_wanted) == 0
    }

    pub(crate) fn scan_space_up_to(&mut self, spaces_wanted: usize) -> usize {
        spaces_wanted - self.scan_space_inner(spaces_wanted)
    }

    fn scan_space_inner(&mut self, mut spaces_wanted: usize) -> usize {
        let from_remaining = self.spaces_remaining.min(spaces_wanted);
        self.spaces_remaining -= from_remaining;
        spaces_wanted -= from_remaining;

        while spaces_wanted > 0 && self.index < self.bytes.len() {
            match self.bytes[self.index] {
                b' ' => {
                    self.index += 1;
                    spaces_wanted -= 1;
                }
                b'\t' => {
                    let spaces =
                        TAB_STOP_COLUMNS - (self.index - self.tab_start) % TAB_STOP_COLUMNS;
                    self.index += 1;
                    self.tab_start = self.index;
                    let count = spaces.min(spaces_wanted);
                    spaces_wanted -= count;

                    self.spaces_remaining = spaces - count;
                }
                _ => break,
            }
        }
        spaces_wanted
    }

    pub(crate) fn scan_all_space(&mut self) {
        self.spaces_remaining = 0;
        self.index += self.bytes[self.index..]
            .iter()
            .take_while(|&&byte| byte == b' ' || byte == b'\t')
            .count();
    }

    pub(crate) fn is_at_end_of_line(&self) -> bool {
        self.bytes
            .get(self.index)
            .map(|&byte| byte == b'\r' || byte == b'\n')
            .unwrap_or(true)
    }

    const fn scan_character(&mut self, byte: u8) -> bool {
        if self.index < self.bytes.len() && self.bytes[self.index] == byte {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn scan_case_insensitive(&mut self, tag: &[u8]) -> bool {
        if self.bytes.len() - self.index < tag.len() {
            return false;
        }
        let prefix = &self.bytes[self.index..self.index + tag.len()];
        let ok = prefix.eq_ignore_ascii_case(tag);
        if ok {
            self.index += tag.len();
        }
        ok
    }

    pub(crate) fn scan_blockquote_tag(&mut self) -> Option<BlockQuoteKind> {
        let saved_index = self.index;
        let tag = if self.scan_character(b'[') && self.scan_character(b'!') {
            let tag = if self.scan_case_insensitive(b"note") {
                Some(BlockQuoteKind::Note)
            } else if self.scan_case_insensitive(b"tip") {
                Some(BlockQuoteKind::Tip)
            } else if self.scan_case_insensitive(b"important") {
                Some(BlockQuoteKind::Important)
            } else if self.scan_case_insensitive(b"warning") {
                Some(BlockQuoteKind::Warning)
            } else if self.scan_case_insensitive(b"caution") {
                Some(BlockQuoteKind::Caution)
            } else {
                None
            };
            if tag.is_some() && self.scan_character(b']') {
                if let Some(newline) = scan_blank_line(&self.bytes[self.index..]) {
                    self.index += newline;
                    tag
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if tag.is_none() {
            self.index = saved_index;
        }
        tag
    }

    pub(crate) fn scan_blockquote_marker(&mut self) -> bool {
        if self.scan_character(b'>') {
            let _ = self.scan_space(1);
            true
        } else {
            false
        }
    }

    pub(crate) fn scan_list_marker_with_indent(
        &mut self,
        indent: usize,
    ) -> Option<(u8, u64, usize)> {
        let save = self.clone();
        if self.index < self.bytes.len() {
            let byte = self.bytes[self.index];
            if byte == b'-' || byte == b'+' || byte == b'*' {
                if self.index >= self.min_thematic_break_offset {
                    if let Err(min_offset) = scan_thematic_break(&self.bytes[self.index..]) {
                        self.min_thematic_break_offset = min_offset;
                    } else {
                        *self = save;
                        return None;
                    }
                }
                self.index += 1;
                if self.scan_space(1) || self.is_at_end_of_line() {
                    return self.finish_list_marker(byte, 0, indent + 2);
                }
            } else if byte.is_ascii_digit() {
                let start_index = self.index;
                let mut index = self.index + 1;
                let mut start_number = u64::from(byte - b'0');
                while index < self.bytes.len()
                    && index - start_index < ORDERED_LIST_MARKER_BYTES_MAX
                {
                    let byte = self.bytes[index];
                    index += 1;
                    if byte.is_ascii_digit() {
                        start_number = start_number * 10 + u64::from(byte - b'0');
                    } else if byte == b')' || byte == b'.' {
                        self.index = index;
                        if self.scan_space(1) || self.is_at_end_of_line() {
                            return self.finish_list_marker(
                                byte,
                                start_number,
                                indent + 1 + index - start_index,
                            );
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        *self = save;
        None
    }

    fn finish_list_marker(
        &mut self,
        byte: u8,
        start: u64,
        mut indent: usize,
    ) -> Option<(u8, u64, usize)> {
        let save = self.clone();

        if scan_blank_line(&self.bytes[self.index..]).is_some() {
            return Some((byte, start, indent));
        }

        let post_indent = self.scan_space_up_to(CODE_INDENT_COLUMNS);
        if post_indent < CODE_INDENT_COLUMNS {
            indent += post_indent;
        } else {
            *self = save;
        }
        Some((byte, start, indent))
    }

    pub(crate) fn scan_task_list_marker(&mut self) -> Option<bool> {
        let save = self.clone();
        self.scan_space_up_to(BLOCK_INDENT_COLUMNS_MAX);

        if !self.scan_character(b'[') {
            *self = save;
            return None;
        }
        let is_checked = match self.bytes.get(self.index) {
            Some(&byte) if is_ascii_whitespace_no_newline(byte) => {
                self.index += 1;
                false
            }
            Some(b'x') | Some(b'X') => {
                self.index += 1;
                true
            }
            _ => {
                *self = save;
                return None;
            }
        };
        if !self.scan_character(b']') {
            *self = save;
            return None;
        }
        if !self
            .bytes
            .get(self.index)
            .map(|&other_byte| is_ascii_whitespace(other_byte))
            .unwrap_or(false)
        {
            *self = save;
            return None;
        }
        Some(is_checked)
    }

    pub(crate) const fn bytes_scanned(&self) -> usize {
        self.index
    }

    pub(crate) const fn remaining_space(&self) -> usize {
        self.spaces_remaining
    }
}

pub(crate) fn is_ascii_whitespace(byte: u8) -> bool {
    (0x09..=0x0d).contains(&byte) || byte == b' '
}

pub(crate) const fn is_ascii_whitespace_no_newline(byte: u8) -> bool {
    byte == b'\t' || byte == 0x0b || byte == 0x0c || byte == b' '
}

const fn is_ascii_alpha(byte: u8) -> bool {
    byte.is_ascii_alphabetic()
}

const fn is_ascii_alphanumeric(byte: u8) -> bool {
    matches!(byte, b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
}

const fn is_ascii_letter_digit_or_dash(byte: u8) -> bool {
    byte == b'-' || is_ascii_alphanumeric(byte)
}

const fn is_valid_unquoted_attribute_value_char(byte: u8) -> bool {
    !matches!(
        byte,
        b'\'' | b'"' | b' ' | b'=' | b'>' | b'<' | b'`' | b'\n' | b'\r'
    )
}

pub(crate) const fn scan_character(data: &[u8], byte: u8) -> usize {
    if !data.is_empty() && data[0] == byte {
        1
    } else {
        0
    }
}

pub(crate) fn scan_while<F>(data: &[u8], mut predicate: F) -> usize
where
    F: FnMut(u8) -> bool,
{
    data.iter().take_while(|&&byte| predicate(byte)).count()
}

pub(crate) fn scan_rev_while<F>(data: &[u8], mut predicate: F) -> usize
where
    F: FnMut(u8) -> bool,
{
    data.iter()
        .rev()
        .take_while(|&&byte| predicate(byte))
        .count()
}

pub(crate) fn scan_character_repeat(data: &[u8], byte: u8) -> usize {
    scan_while(data, |offset| offset == byte)
}

pub(crate) fn scan_whitespace_no_newline(data: &[u8]) -> usize {
    scan_while(data, is_ascii_whitespace_no_newline)
}

fn scan_attribute_value_chars(data: &[u8]) -> usize {
    scan_while(data, is_valid_unquoted_attribute_value_char)
}

pub(crate) const fn scan_end_of_line(bytes: &[u8]) -> Option<usize> {
    match *bytes {
        [] => Some(0),
        [b'\n', ..] => Some(1),
        [b'\r', b'\n', ..] => Some(2),
        [b'\r', ..] => Some(1),
        _ => None,
    }
}

pub(crate) fn scan_blank_line(bytes: &[u8]) -> Option<usize> {
    let cursor = scan_whitespace_no_newline(bytes);
    scan_end_of_line(&bytes[cursor..]).map(|count| cursor + count)
}

pub(crate) fn scan_nextline(bytes: &[u8]) -> usize {
    memchr(b'\n', bytes).map_or(bytes.len(), |offset| offset + 1)
}

pub(crate) fn scan_closing_code_fence(
    bytes: &[u8],
    fence_char: u8,
    fence_length: usize,
) -> Option<usize> {
    if bytes.is_empty() {
        return Some(0);
    }
    let mut cursor = 0;
    let number_fence_chars_found = scan_character_repeat(&bytes[cursor..], fence_char);
    if number_fence_chars_found < fence_length {
        return None;
    }
    cursor += number_fence_chars_found;
    let number_trailing_spaces = scan_character_repeat(&bytes[cursor..], b' ');
    cursor += number_trailing_spaces;
    scan_end_of_line(&bytes[cursor..]).map(|_| cursor)
}

pub(crate) fn calculate_indent(text: &[u8], max: usize) -> (usize, usize) {
    let mut spaces = 0;
    let mut offset = 0;

    for (cursor, &byte) in text.iter().enumerate() {
        offset = cursor;
        match byte {
            b' ' => {
                spaces += 1;
                if spaces == max {
                    break;
                }
            }
            b'\t' => {
                let new_spaces = spaces + TAB_STOP_COLUMNS - spaces % TAB_STOP_COLUMNS;
                if new_spaces > max {
                    break;
                }
                spaces = new_spaces;
            }
            _ => break,
        }
    }

    (offset, spaces)
}

pub(crate) fn scan_thematic_break(bytes: &[u8]) -> Result<usize, usize> {
    if bytes.len() < THEMATIC_BREAK_MARKERS_MIN {
        return Err(0);
    }
    let byte = bytes[0];
    if !(byte == b'*' || byte == b'-' || byte == b'_') {
        return Err(0);
    }
    let mut count = 0;
    let mut cursor = 0;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\n' | b'\r' => {
                cursor += scan_end_of_line(&bytes[cursor..]).unwrap_or(0);
                break;
            }
            c2 if c2 == byte => {
                count += 1;
            }
            b' ' | b'\t' => (),
            _ => return Err(cursor),
        }
        cursor += 1;
    }
    if count >= THEMATIC_BREAK_MARKERS_MIN {
        Ok(cursor)
    } else {
        Err(cursor)
    }
}

pub(crate) fn scan_atx_heading(data: &[u8]) -> Option<HeadingLevel> {
    let level = scan_character_repeat(data, b'#');
    if data.get(level).copied().is_none_or(is_ascii_whitespace) {
        heading_level(level)
    } else {
        None
    }
}

const fn heading_level(level: usize) -> Option<HeadingLevel> {
    match level {
        1 => Some(HeadingLevel::H1),
        2 => Some(HeadingLevel::H2),
        3 => Some(HeadingLevel::H3),
        4 => Some(HeadingLevel::H4),
        5 => Some(HeadingLevel::H5),
        6 => Some(HeadingLevel::H6),
        _ => None,
    }
}

pub(crate) fn scan_setext_heading(data: &[u8]) -> Option<(usize, HeadingLevel)> {
    let byte = *data.first()?;
    let level = if byte == b'=' {
        HeadingLevel::H1
    } else if byte == b'-' {
        HeadingLevel::H2
    } else {
        return None;
    };
    let mut cursor = 1 + scan_character_repeat(&data[1..], byte);
    cursor += scan_blank_line(&data[cursor..])?;
    Some((cursor, level))
}

pub(crate) fn scan_table_head(data: &[u8]) -> (usize, Vec<Alignment>) {
    let (mut cursor, spaces) = calculate_indent(data, CODE_INDENT_COLUMNS);
    if spaces > BLOCK_INDENT_COLUMNS_MAX || cursor == data.len() {
        return (0, vec![]);
    }
    let mut columns = vec![];
    let mut active_col = Alignment::None;
    let mut start_col = true;
    let mut found_pipe = false;
    let mut found_hyphen = false;
    let mut found_hyphen_in_col = false;
    if data[cursor] == b'|' {
        cursor += 1;
        found_pipe = true;
    }
    for byte in &data[cursor..] {
        if let Some(count) = scan_end_of_line(&data[cursor..]) {
            cursor += count;
            break;
        }
        match *byte {
            b' ' => (),
            b':' => {
                active_col = match (start_col, active_col) {
                    (true, Alignment::None) => Alignment::Left,
                    (false, Alignment::Left) => Alignment::Center,
                    (false, Alignment::None) => Alignment::Right,
                    _ => active_col,
                };
                start_col = false;
            }
            b'-' => {
                start_col = false;
                found_hyphen = true;
                found_hyphen_in_col = true;
            }
            b'|' => {
                start_col = true;
                found_pipe = true;
                columns.push(active_col);
                active_col = Alignment::None;
                if !found_hyphen_in_col {
                    return (0, vec![]);
                }
                found_hyphen_in_col = false;
            }
            _ => {
                return (0, vec![]);
            }
        }
        cursor += 1;
    }

    if !start_col {
        columns.push(active_col);
    }
    if !found_pipe || !found_hyphen {
        return (0, vec![]);
    }

    (cursor, columns)
}

pub(crate) fn scan_code_fence(data: &[u8]) -> Option<(usize, u8)> {
    let byte = *data.first()?;
    if !(byte == b'`' || byte == b'~') {
        return None;
    }
    let cursor = 1 + scan_character_repeat(&data[1..], byte);
    if cursor >= FENCE_LENGTH_MIN {
        if byte == b'`' {
            let suffix = &data[cursor..];
            let next_line = cursor + scan_nextline(suffix);

            if suffix[..(next_line - cursor)].contains(&b'`') {
                return None;
            }
        }
        Some((cursor, byte))
    } else {
        None
    }
}

pub(crate) fn scan_blockquote_start(data: &[u8]) -> Option<usize> {
    if data.first().copied() == Some(b'>') {
        let space = if data.get(1).copied() == Some(b' ') {
            1
        } else {
            0
        };
        Some(1 + space)
    } else {
        None
    }
}

pub(crate) fn scan_list_item(bytes: &[u8]) -> Option<(usize, u8, usize, usize)> {
    let mut byte = *bytes.first()?;
    let (marker_width, start) = match byte {
        b'-' | b'+' | b'*' => (1, 0),
        b'0'..=b'9' => {
            let (length, start) = parse_decimal(bytes, ORDERED_LIST_DIGITS_MAX);
            byte = *bytes.get(length)?;
            if !(byte == b'.' || byte == b')') {
                return None;
            }
            (length + 1, start)
        }
        _ => {
            return None;
        }
    };

    let (mut post_bytes, mut post_columns) =
        calculate_indent(&bytes[marker_width..], CODE_INDENT_COLUMNS + 1);
    if post_columns == 0 {
        scan_end_of_line(&bytes[marker_width..])?;
        post_columns += 1;
    } else if post_columns > CODE_INDENT_COLUMNS {
        post_bytes = 1;
        post_columns = 1;
    }
    if scan_blank_line(&bytes[marker_width..]).is_some() {
        post_bytes = 0;
        post_columns = 1;
    }
    Some((
        marker_width + post_bytes,
        byte,
        start,
        marker_width + post_columns,
    ))
}

fn parse_decimal(bytes: &[u8], limit: usize) -> (usize, usize) {
    parse_digits(bytes, limit, DECIMAL_RADIX)
}

fn parse_hex(bytes: &[u8], limit: usize) -> (usize, usize) {
    parse_digits(bytes, limit, HEX_RADIX)
}

fn parse_digits(bytes: &[u8], limit: usize, radix: u32) -> (usize, usize) {
    let mut count = 0;
    let mut accumulated = 0_usize;
    for &byte in bytes.iter().take(limit) {
        let Some(digit) = char::from(byte).to_digit(radix) else {
            break;
        };
        let Some(number) = accumulated
            .checked_mul(radix as usize)
            .and_then(|scaled| scaled.checked_add(digit as usize))
        else {
            break;
        };
        accumulated = number;
        count += 1;
    }
    (count, accumulated)
}

fn character_from_code_point(input: usize) -> Option<char> {
    let code_point = input.try_into().ok()?;
    if code_point == 0 {
        return None;
    }
    char::from_u32(code_point)
}

pub(crate) fn scan_entity(bytes: &[u8]) -> (usize, Option<CowStr<'static>>) {
    let mut end = 1;
    if bytes.get(end) == Some(&b'#') {
        end += 1;
        let (byte_count, code_point) =
            if end < bytes.len() && bytes[end].eq_ignore_ascii_case(&b'x') {
                end += 1;
                parse_hex(&bytes[end..], ENTITY_HEX_DIGITS_MAX)
            } else {
                parse_decimal(&bytes[end..], ENTITY_DECIMAL_DIGITS_MAX)
            };
        end += byte_count;
        return if byte_count == 0 || bytes.get(end) != Some(&b';') {
            (0, None)
        } else {
            (
                end + 1,
                Some(
                    character_from_code_point(code_point)
                        .unwrap_or(char::REPLACEMENT_CHARACTER)
                        .into(),
                ),
            )
        };
    }
    end += scan_while(&bytes[end..], is_ascii_alphanumeric);
    if bytes.get(end) == Some(&b';')
        && let Some(value) = entities::entity(&bytes[1..end])
    {
        return (end + 1, Some(value.into()));
    }
    (0, None)
}

pub(crate) fn scan_link_destination(
    data: &str,
    start_index: usize,
    max_next: usize,
) -> Option<(usize, &str)> {
    let bytes = &data.as_bytes()[start_index..];
    let mut cursor = scan_character(bytes, b'<');

    if cursor != 0 {
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'\n' | b'\r' | b'<' => return None,
                b'>' => {
                    return Some((cursor + 1, &data[(start_index + 1)..(start_index + cursor)]));
                }
                b'\\' if cursor + 1 < bytes.len() && is_ascii_punctuation(bytes[cursor + 1]) => {
                    cursor += 1;
                }
                _ => {}
            }
            cursor += 1;
        }
        None
    } else {
        let mut nest = 0;
        while cursor < bytes.len() {
            match bytes[cursor] {
                0x0..=0x20 => {
                    break;
                }
                b'(' => {
                    if nest > max_next {
                        return None;
                    }
                    nest += 1;
                }
                b')' => {
                    if nest == 0 {
                        break;
                    }
                    nest -= 1;
                }
                b'\\' if cursor + 1 < bytes.len() && is_ascii_punctuation(bytes[cursor + 1]) => {
                    cursor += 1;
                }
                _ => {}
            }
            cursor += 1;
        }
        if nest != 0 {
            return None;
        }
        Some((cursor, &data[start_index..(start_index + cursor)]))
    }
}

fn scan_attribute_name(data: &[u8]) -> Option<usize> {
    let (&byte, tail) = data.split_first()?;
    if is_ascii_alpha(byte) || byte == b'_' || byte == b':' {
        Some(
            1 + scan_while(tail, |byte| {
                is_ascii_alphanumeric(byte)
                    || byte == b'_'
                    || byte == b'.'
                    || byte == b':'
                    || byte == b'-'
            }),
        )
    } else {
        None
    }
}

fn scan_attribute(
    data: &[u8],
    mut index: usize,
    newline_handler: NewlineHandler<'_>,
    buffer: &mut Vec<u8>,
    buffer_index: &mut usize,
) -> Option<usize> {
    index += scan_attribute_name(&data[index..])?;
    let index_after_attribute = index;
    index = scan_whitespace_with_newline_handler_without_buffer(data, index, newline_handler)?;
    if data.get(index) == Some(&b'=') {
        index = scan_whitespace_with_newline_handler(
            data,
            index_after_attribute,
            newline_handler,
            buffer,
            buffer_index,
        )?;
        index += 1;
        index = scan_whitespace_with_newline_handler(
            data,
            index,
            newline_handler,
            buffer,
            buffer_index,
        )?;
        index = scan_attribute_value(data, index, newline_handler, buffer, buffer_index)?;
        Some(index)
    } else {
        Some(index_after_attribute)
    }
}

fn scan_whitespace_with_newline_handler(
    data: &[u8],
    mut cursor: usize,
    newline_handler: NewlineHandler<'_>,
    buffer: &mut Vec<u8>,
    buffer_index: &mut usize,
) -> Option<usize> {
    while cursor < data.len() {
        if !is_ascii_whitespace(data[cursor]) {
            return Some(cursor);
        }
        if let Some(end_of_line_bytes) = scan_end_of_line(&data[cursor..]) {
            let handler = newline_handler?;
            cursor += end_of_line_bytes;
            let skipped_bytes = handler(&data[cursor..]);

            if skipped_bytes > 0 {
                buffer.extend(&data[*buffer_index..cursor]);
                *buffer_index = cursor + skipped_bytes;
            }

            cursor += skipped_bytes;
        } else {
            cursor += 1;
        }
    }

    Some(cursor)
}

fn scan_whitespace_with_newline_handler_without_buffer(
    data: &[u8],
    mut cursor: usize,
    newline_handler: NewlineHandler<'_>,
) -> Option<usize> {
    while cursor < data.len() {
        if !is_ascii_whitespace(data[cursor]) {
            return Some(cursor);
        }
        if let Some(end_of_line_bytes) = scan_end_of_line(&data[cursor..]) {
            let handler = newline_handler?;
            cursor += end_of_line_bytes;
            let skipped_bytes = handler(&data[cursor..]);
            cursor += skipped_bytes;
        } else {
            cursor += 1;
        }
    }

    Some(cursor)
}

fn scan_attribute_value(
    data: &[u8],
    mut cursor: usize,
    newline_handler: NewlineHandler<'_>,
    buffer: &mut Vec<u8>,
    buffer_index: &mut usize,
) -> Option<usize> {
    match *data.get(cursor)? {
        byte @ b'"' | byte @ b'\'' => {
            cursor += 1;
            while cursor < data.len() {
                if data[cursor] == byte {
                    return Some(cursor + 1);
                }
                if let Some(end_of_line_bytes) = scan_end_of_line(&data[cursor..]) {
                    let handler = newline_handler?;
                    cursor += end_of_line_bytes;
                    let skipped_bytes = handler(&data[cursor..]);

                    if skipped_bytes > 0 {
                        buffer.extend(&data[*buffer_index..cursor]);
                        *buffer_index = cursor + skipped_bytes;
                    }
                    cursor += skipped_bytes;
                } else {
                    cursor += 1;
                }
            }
            return None;
        }
        b' ' | b'=' | b'>' | b'<' | b'`' | b'\n' | b'\r' => {
            return None;
        }
        _ => {
            cursor += scan_attribute_value_chars(&data[cursor..]);
        }
    }

    Some(cursor)
}

pub(crate) fn unescape<'a, I: Into<CowStr<'a>>>(input: I, is_in_table: bool) -> CowStr<'a> {
    let input = input.into();
    let mut result = String::new();
    let mut mark = 0;
    let mut cursor = 0;
    let bytes = input.as_bytes();
    while cursor < bytes.len() {
        match bytes[cursor..] {
            [b'\\', b'\\', b'|', ..] if is_in_table => {
                result.push_str(&input[mark..cursor]);
                mark = cursor + 2;
                cursor += 3;
            }
            [b'\\', next_byte, ..] if is_ascii_punctuation(next_byte) => {
                result.push_str(&input[mark..cursor]);
                mark = cursor + 1;
                cursor += 2;
            }
            [b'&', ..] => match scan_entity(&bytes[cursor..]) {
                (count, Some(value)) => {
                    result.push_str(&input[mark..cursor]);
                    result.push_str(&value);
                    cursor += count;
                    mark = cursor;
                }
                _ => cursor += 1,
            },
            [b'\r', ..] => {
                result.push_str(&input[mark..cursor]);
                cursor += 1;
                mark = cursor;
            }
            _ => cursor += 1,
        }
    }
    if mark == 0 {
        input
    } else {
        result.push_str(&input[mark..]);
        result.into()
    }
}

pub(crate) fn starts_html_block_type_6(data: &[u8]) -> bool {
    let cursor = scan_character(data, b'/');
    let tail = &data[cursor..];
    let count = scan_while(tail, is_ascii_alphanumeric);
    if !is_html_tag(&tail[..count]) {
        return false;
    }

    let tail = &tail[count..];
    tail.is_empty()
        || tail[0] == b' '
        || tail[0] == b'\t'
        || tail[0] == b'\r'
        || tail[0] == b'\n'
        || tail[0] == b'>'
        || tail.len() >= 2 && &tail[..2] == b"/>"
}

fn is_html_tag(tag: &[u8]) -> bool {
    HTML_TAGS
        .binary_search_by(|probe| {
            let probe_bytes_iter = probe.as_bytes().iter();
            let tag_bytes_iter = tag.iter();

            probe_bytes_iter
                .zip(tag_bytes_iter)
                .find_map(
                    |(&left, &byte)| match left.cmp(&byte.to_ascii_lowercase()) {
                        std::cmp::Ordering::Equal => None,
                        inequality => Some(inequality),
                    },
                )
                .unwrap_or_else(|| probe.len().cmp(&tag.len()))
        })
        .is_ok()
}

pub(crate) fn scan_html_type_7(data: &[u8]) -> Option<usize> {
    let (_span, cursor) = scan_html_block_inner(data, None)?;
    scan_blank_line(&data[cursor..])?;
    Some(cursor)
}

pub(crate) fn scan_html_block_inner(
    data: &[u8],
    newline_handler: NewlineHandler<'_>,
) -> Option<(Vec<u8>, usize)> {
    let mut buffer = Vec::new();
    let mut last_buffer_index = 0;

    let close_tag_bytes = scan_character(&data[1..], b'/');
    let length = scan_while(&data[(1 + close_tag_bytes)..], is_ascii_alpha);
    if length == 0 {
        return None;
    }
    let mut cursor = 1 + close_tag_bytes + length;
    cursor += scan_while(&data[cursor..], is_ascii_letter_digit_or_dash);

    if close_tag_bytes == 0 {
        loop {
            let previous_cursor = cursor;
            loop {
                cursor += scan_whitespace_no_newline(&data[cursor..]);
                if let Some(end_of_line_bytes) = scan_end_of_line(&data[cursor..]) {
                    if end_of_line_bytes == 0 {
                        return None;
                    }
                    let handler = newline_handler?;
                    cursor += end_of_line_bytes;
                    let skipped_bytes = handler(&data[cursor..]);

                    let data_length = data.len() - cursor;

                    assert!(
                        skipped_bytes <= data_length,
                        "Handler tried to skip too many bytes, fed {}, skipped {}",
                        data_length,
                        skipped_bytes
                    );

                    if skipped_bytes > 0 {
                        buffer.extend(&data[last_buffer_index..cursor]);
                        cursor += skipped_bytes;
                        last_buffer_index = cursor;
                    }
                } else {
                    break;
                }
            }
            if let Some(b'/') | Some(b'>') = data.get(cursor) {
                break;
            }
            if previous_cursor == cursor {
                return None;
            }
            cursor = scan_attribute(
                data,
                cursor,
                newline_handler,
                &mut buffer,
                &mut last_buffer_index,
            )?;
        }
    }

    cursor += scan_whitespace_no_newline(&data[cursor..]);

    if close_tag_bytes == 0 {
        cursor += scan_character(&data[cursor..], b'/');
    }

    if data.get(cursor) != Some(&b'>') {
        None
    } else {
        cursor += 1;
        if !buffer.is_empty() {
            buffer.extend(&data[last_buffer_index..cursor]);
        }
        Some((buffer, cursor))
    }
}

pub(crate) fn scan_autolink(
    text: &str,
    start_index: usize,
) -> Option<(usize, CowStr<'_>, LinkType)> {
    scan_uri(text, start_index)
        .map(|(bytes, uri)| (bytes, uri, LinkType::Autolink))
        .or_else(|| scan_email(text, start_index).map(|(bytes, uri)| (bytes, uri, LinkType::Email)))
}

fn scan_uri(text: &str, start_index: usize) -> Option<(usize, CowStr<'_>)> {
    let bytes = &text.as_bytes()[start_index..];

    if bytes.is_empty() || !is_ascii_alpha(bytes[0]) {
        return None;
    }

    let mut cursor = 1;

    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor += 1;
        match byte {
            byte if is_ascii_alphanumeric(byte) => (),
            b'.' | b'-' | b'+' => (),
            b':' => break,
            _ => return None,
        }
    }

    if !(URI_SCHEME_LENGTH_MIN + 1..=URI_SCHEME_LENGTH_MAX + 1).contains(&cursor) {
        return None;
    }

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'>' => {
                return Some((
                    start_index + cursor + 1,
                    text[start_index..(start_index + cursor)].into(),
                ));
            }
            b'\0'..=b' ' | b'<' => return None,
            _ => (),
        }
        cursor += 1;
    }

    None
}

fn scan_email(text: &str, start_index: usize) -> Option<(usize, CowStr<'_>)> {
    let bytes = &text.as_bytes()[start_index..];
    let mut cursor = 0;

    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor += 1;
        match byte {
            byte if is_ascii_alphanumeric(byte) => (),
            b'.' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'/' | b'=' | b'?'
            | b'^' | b'_' | b'`' | b'{' | b'|' | b'}' | b'~' | b'-' => (),
            b'@' if cursor > 1 => break,
            _ => return None,
        }
    }

    loop {
        let label_start_index = cursor;
        let mut fresh_label = true;

        while cursor < bytes.len() {
            match bytes[cursor] {
                byte if is_ascii_alphanumeric(byte) => (),
                b'-' if fresh_label => {
                    return None;
                }
                b'-' => (),
                _ => break,
            }
            fresh_label = false;
            cursor += 1;
        }

        if cursor == label_start_index
            || cursor - label_start_index > EMAIL_LABEL_LENGTH_MAX
            || bytes[cursor - 1] == b'-'
        {
            return None;
        }

        if bytes.get(cursor) != Some(&b'.') {
            break;
        }
        cursor += 1;
    }

    if bytes.get(cursor) != Some(&b'>') {
        return None;
    }

    Some((
        start_index + cursor + 1,
        text[start_index..(start_index + cursor)].into(),
    ))
}

pub(crate) fn scan_inline_html_comment(
    bytes: &[u8],
    mut index: usize,
    scan_guard: &mut HtmlScanGuard,
) -> Option<usize> {
    let byte = *bytes.get(index)?;
    index += 1;
    match byte {
        b'-' if index > scan_guard.comment => {
            if *bytes.get(index)? != b'-' {
                return None;
            }

            index -= 1;

            while let Some(offset) = memchr(b'-', &bytes[index..]) {
                index += offset + 1;
                scan_guard.comment = index;
                if bytes.get(index) == Some(&b'-') && bytes.get(index + 1) == Some(&b'>') {
                    return Some(index + 2);
                }
            }
            None
        }

        b'[' if bytes[index..].starts_with(b"CDATA[") && index > scan_guard.cdata => {
            index += b"CDATA[".len();
            index = memchr(b']', &bytes[index..]).map_or(bytes.len(), |offset| index + offset);
            let close_brackets = scan_character_repeat(&bytes[index..], b']');
            index += close_brackets;

            if close_brackets == 0 || bytes.get(index) != Some(&b'>') {
                scan_guard.cdata = index;
                None
            } else {
                Some(index + 1)
            }
        }

        _ if byte.is_ascii_alphabetic() && index > scan_guard.declaration => {
            index = memchr(b'>', &bytes[index..]).map_or(bytes.len(), |offset| index + offset);
            if bytes.get(index) != Some(&b'>') {
                scan_guard.declaration = index;
                None
            } else {
                Some(index + 1)
            }
        }
        _ => None,
    }
}

pub(crate) fn scan_inline_html_processing(
    bytes: &[u8],
    mut index: usize,
    scan_guard: &mut HtmlScanGuard,
) -> Option<usize> {
    if index <= scan_guard.processing {
        return None;
    }
    while let Some(offset) = memchr(b'?', &bytes[index..]) {
        index += offset + 1;
        if bytes.get(index) == Some(&b'>') {
            return Some(index + 1);
        }
    }
    scan_guard.processing = index;
    None
}
