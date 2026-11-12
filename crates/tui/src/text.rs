use std::ops::Range;

const BLOCK_BYTES: usize = 32;
const PRINTABLE_FIRST: u8 = b' ';
const PRINTABLE_LAST: u8 = b'~';
const DELETE: u8 = 0x7f;
const C1_LEAD: u8 = 0xc2;
const C1_CONTINUATION_END: u8 = 0xa0;

fn first(bytes: &[u8], candidate: impl Fn(u8) -> bool, matches: impl Fn(usize) -> bool) -> usize {
    let (blocks, _) = bytes.as_chunks::<BLOCK_BYTES>();
    let mut offset = 0;
    for block in blocks {
        if block.iter().fold(false, |any, byte| any | candidate(*byte))
            && let Some(found) = (offset..offset + BLOCK_BYTES).find(|index| matches(*index))
        {
            return found;
        }
        offset += BLOCK_BYTES;
    }
    (offset..bytes.len())
        .find(|index| matches(*index))
        .unwrap_or(bytes.len())
}

pub(crate) fn ascii_prefix(text: &str, newline: bool) -> usize {
    let bytes = text.as_bytes();
    first(
        bytes,
        |byte| !(PRINTABLE_FIRST..=PRINTABLE_LAST).contains(&byte),
        |index| {
            let byte = bytes[index];
            !(PRINTABLE_FIRST..=PRINTABLE_LAST).contains(&byte) && !(newline && byte == b'\n')
        },
    )
}

pub(crate) fn control_prefix(text: &str, newline: bool) -> usize {
    let bytes = text.as_bytes();
    first(
        bytes,
        |byte| (byte < PRINTABLE_FIRST) | (byte == DELETE) | (byte == C1_LEAD),
        |index| {
            let byte = bytes[index];
            (byte < PRINTABLE_FIRST || byte == DELETE) && !(newline && byte == b'\n')
                || (byte == C1_LEAD && bytes[index + 1] < C1_CONTINUATION_END)
        },
    )
}

pub(crate) fn ascii_rows(
    text: &str,
    width: usize,
    prefer_words: bool,
    preserve_whitespace: bool,
) -> Vec<(Range<usize>, usize, usize)> {
    let bytes = text.as_bytes();
    let mut rows = Vec::new();
    let mut line_start = 0;
    for (line, line_end) in memchr::memchr_iter(b'\n', bytes)
        .chain(std::iter::once(bytes.len()))
        .enumerate()
    {
        if line_start == line_end {
            rows.push((line_start..line_end, line, line_end));
        }
        let mut start = line_start;
        while start < line_end {
            if preserve_whitespace && start > line_start && bytes[start] == b' ' {
                start += 1;
            }
            if !preserve_whitespace {
                while start < line_end && bytes[start] == b' ' {
                    start += 1;
                }
            }
            if start == line_end {
                break;
            }
            let end = start.saturating_add(width).min(line_end);
            let split = if prefer_words && end < line_end {
                let first_content = if preserve_whitespace {
                    bytes[start..line_end]
                        .iter()
                        .position(|byte| *byte != b' ')
                        .map_or(start, |offset| start + offset)
                } else {
                    start
                };
                memchr::memrchr(b' ', &bytes[start..end])
                    .map(|offset| start + offset)
                    .filter(|index| {
                        *index > start && (!preserve_whitespace || *index > first_content)
                    })
                    .unwrap_or(end)
            } else {
                end
            };
            let rendered = if preserve_whitespace {
                split
            } else {
                start + bytes[start..split].trim_ascii_end().len()
            };
            rows.push((start..rendered, line, end));
            start = split;
        }
        line_start = line_end + 1;
    }
    rows
}
