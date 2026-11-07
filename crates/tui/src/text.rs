use std::ops::Range;
use wide::u8x16;

pub(crate) fn ascii_prefix(text: &str, newline: bool) -> usize {
    prefix(
        text,
        |values| {
            let special = values.simd_lt(u8x16::splat(b' ')) | values.simd_gt(u8x16::splat(b'~'));
            if newline {
                special & !values.simd_eq(u8x16::splat(b'\n'))
            } else {
                special
            }
        },
        |byte| !(b' '..=b'~').contains(&byte) && !(newline && byte == b'\n'),
    )
}

pub(crate) fn control_prefix(text: &str, newline: bool) -> usize {
    let offset = prefix(
        text,
        |values| {
            let special = values.simd_lt(u8x16::splat(b' '))
                | values.simd_eq(u8x16::splat(0x7f))
                | values.simd_eq(u8x16::splat(0xc2));
            if newline {
                special & !values.simd_eq(u8x16::splat(b'\n'))
            } else {
                special
            }
        },
        |byte| (byte < b' ' || byte == 0x7f || byte == 0xc2) && !(newline && byte == b'\n'),
    );
    if text.as_bytes().get(offset) == Some(&0xc2) && text.as_bytes()[offset + 1] >= 0xa0 {
        offset + control_prefix_unicode(&text[offset..], newline)
    } else {
        offset
    }
}

fn control_prefix_unicode(text: &str, newline: bool) -> usize {
    let bytes = text.as_bytes();
    let mut offset = 0;
    let (chunks, remainder) = bytes.as_chunks::<{ u8x16::LANES as usize }>();
    for chunk in chunks {
        let values = u8x16::from(*chunk);
        let c2 = values.simd_eq(u8x16::splat(0xc2));
        let mut special =
            values.simd_lt(u8x16::splat(b' ')) | values.simd_eq(u8x16::splat(0x7f)) | c2;
        if newline {
            special &= !values.simd_eq(u8x16::splat(b'\n'));
        }
        if special.any() {
            let next = values.shuffle(u8x16::new([
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 15,
            ]));
            special &= !c2 | next.simd_lt(u8x16::splat(0xa0));
            if special.any() {
                return offset + special.to_bitmask().trailing_zeros() as usize;
            }
            let next_offset = offset + u8x16::LANES as usize;
            if chunk[chunk.len() - 1] == 0xc2 && bytes[next_offset] < 0xa0 {
                return next_offset - 1;
            }
        }
        offset += u8x16::LANES as usize;
    }
    offset
        + remainder
            .iter()
            .enumerate()
            .position(|(index, byte)| {
                (*byte < b' ' || *byte == 0x7f) && !(newline && *byte == b'\n')
                    || (*byte == 0xc2 && bytes[offset + index + 1] < 0xa0)
            })
            .unwrap_or(remainder.len())
}

fn prefix(text: &str, special: impl Fn(u8x16) -> u8x16, scalar: impl Fn(u8) -> bool) -> usize {
    let bytes = text.as_bytes();
    let mut offset = 0;
    let (chunks, remainder) = bytes.as_chunks::<{ u8x16::LANES as usize }>();
    for chunk in chunks {
        let mask = special(u8x16::from(*chunk));
        if mask.any() {
            return offset + mask.to_bitmask().trailing_zeros() as usize;
        }
        offset += u8x16::LANES as usize;
    }
    offset
        + remainder
            .iter()
            .position(|byte| scalar(*byte))
            .unwrap_or(remainder.len())
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
