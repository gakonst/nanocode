#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    expect(clippy::missing_const_for_fn)
)]
pub fn decompress_block(block: &[u8], size: usize) -> Option<Vec<u8>> {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        let mut output = Vec::with_capacity(size);
        let written = kernel::decode(block, &mut output.spare_capacity_mut()[..size])?;
        assert!(written <= size, "the LZ4 kernel writes inside its output");
        if written != size {
            return None;
        }
        unsafe { output.set_len(size) };
        Some(output)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (block, size);
        None
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod kernel {
    use core::mem::MaybeUninit;
    use core::ptr;

    const MATCH_LENGTH_MIN: usize = 4;
    const NIBBLE_MAX: usize = 0xF;
    const LENGTH_BYTE_MAX: usize = 0xFF;
    const LITERAL_NIBBLE_SHIFT: u32 = 4;
    const OFFSET_BYTES: usize = 2;
    const WILD_COPY_BYTES: usize = 16;
    const OVERLAP_CHUNK_BYTES: usize = 8;
    const SHORT_MATCH_BYTES_MAX: usize = NIBBLE_MAX - 1 + MATCH_LENGTH_MIN;
    const SHORT_MATCH_WRITE_BYTES: usize =
        SHORT_MATCH_BYTES_MAX.next_multiple_of(OVERLAP_CHUNK_BYTES);
    const SHORT_SEQUENCE_INPUT_BYTES: usize = 1 + WILD_COPY_BYTES + OFFSET_BYTES;
    const SHORT_SEQUENCE_OUTPUT_BYTES: usize = NIBBLE_MAX - 1 + SHORT_MATCH_WRITE_BYTES;
    const OVERLAP_PATTERN_INDICES: [[usize; OVERLAP_CHUNK_BYTES]; OVERLAP_CHUNK_BYTES] =
        overlap_pattern_indices();
    const OVERLAP_DISTANCES: [usize; OVERLAP_CHUNK_BYTES] = overlap_distances();

    pub(super) fn decode(block: &[u8], output: &mut [MaybeUninit<u8>]) -> Option<usize> {
        let input = block.as_ptr();
        let base = output.as_mut_ptr().cast::<u8>();
        let size = output.len();
        let mut position = 0;
        let mut written = 0;
        loop {
            let token = *block.get(position)?;
            let literal_nibble = usize::from(token >> LITERAL_NIBBLE_SHIFT);
            let match_nibble = usize::from(token) & NIBBLE_MAX;
            if literal_nibble != NIBBLE_MAX
                && block.len() - position >= SHORT_SEQUENCE_INPUT_BYTES
                && size - written >= SHORT_SEQUENCE_OUTPUT_BYTES
            {
                unsafe {
                    ptr::copy_nonoverlapping(
                        input.add(position + 1),
                        base.add(written),
                        WILD_COPY_BYTES,
                    );
                }
                position += 1 + literal_nibble;
                written += literal_nibble;
                let offset_bytes = unsafe {
                    input
                        .add(position)
                        .cast::<[u8; OFFSET_BYTES]>()
                        .read_unaligned()
                };
                let offset = usize::from(u16::from_le_bytes(offset_bytes));
                position += OFFSET_BYTES;
                if offset == 0 || offset > written {
                    return None;
                }
                if match_nibble != NIBBLE_MAX {
                    let match_bytes = match_nibble + MATCH_LENGTH_MIN;
                    if offset >= match_bytes {
                        unsafe {
                            ptr::copy(
                                base.add(written - offset),
                                base.add(written),
                                SHORT_MATCH_BYTES_MAX,
                            );
                        }
                    } else {
                        unsafe { copy_overlapping(base, size, written, offset, match_bytes) };
                    }
                    written += match_bytes;
                    continue;
                }
                let match_bytes =
                    read_length(block, &mut position, match_nibble)? + MATCH_LENGTH_MIN;
                if match_bytes > size - written {
                    return None;
                }
                unsafe { copy_match(base, size, written, offset, match_bytes) };
                written += match_bytes;
                continue;
            }
            position += 1;
            let literal_bytes = read_length(block, &mut position, literal_nibble)?;
            if literal_bytes > block.len() - position || literal_bytes > size - written {
                return None;
            }
            let wild_bytes = literal_bytes.max(1).next_multiple_of(WILD_COPY_BYTES);
            if block.len() - position >= wild_bytes && size - written >= wild_bytes {
                let mut copied = 0;
                while copied < literal_bytes {
                    unsafe {
                        ptr::copy_nonoverlapping(
                            input.add(position + copied),
                            base.add(written + copied),
                            WILD_COPY_BYTES,
                        );
                    }
                    copied += WILD_COPY_BYTES;
                }
            } else {
                unsafe {
                    ptr::copy_nonoverlapping(input.add(position), base.add(written), literal_bytes);
                }
            }
            written += literal_bytes;
            position += literal_bytes;
            if position == block.len() {
                return Some(written);
            }
            if block.len() - position < OFFSET_BYTES {
                return None;
            }
            let offset = usize::from(u16::from_le_bytes([block[position], block[position + 1]]));
            position += OFFSET_BYTES;
            if offset == 0 || offset > written {
                return None;
            }
            let match_bytes = read_length(block, &mut position, match_nibble)? + MATCH_LENGTH_MIN;
            if match_bytes > size - written {
                return None;
            }
            unsafe { copy_match(base, size, written, offset, match_bytes) };
            written += match_bytes;
        }
    }

    fn read_length(block: &[u8], position: &mut usize, nibble: usize) -> Option<usize> {
        let mut length = nibble;
        if nibble != NIBBLE_MAX {
            return Some(length);
        }
        loop {
            let byte = usize::from(*block.get(*position)?);
            *position += 1;
            length = length.checked_add(byte)?;
            if byte != LENGTH_BYTE_MAX {
                return Some(length);
            }
        }
    }

    #[inline(always)]
    unsafe fn copy_match(
        base: *mut u8,
        size: usize,
        written: usize,
        offset: usize,
        match_bytes: usize,
    ) {
        assert!(
            offset > 0 && offset <= written && match_bytes <= size - written,
            "the match lies inside the output"
        );
        let source = written - offset;
        if offset >= WILD_COPY_BYTES
            && size - written >= match_bytes.next_multiple_of(WILD_COPY_BYTES)
        {
            let mut copied = 0;
            while copied < match_bytes {
                unsafe {
                    ptr::copy_nonoverlapping(
                        base.add(source + copied),
                        base.add(written + copied),
                        WILD_COPY_BYTES,
                    );
                }
                copied += WILD_COPY_BYTES;
            }
            return;
        }
        if offset >= match_bytes {
            unsafe { ptr::copy_nonoverlapping(base.add(source), base.add(written), match_bytes) };
            return;
        }
        if size - written >= match_bytes.next_multiple_of(OVERLAP_CHUNK_BYTES) {
            unsafe { copy_overlapping(base, size, written, offset, match_bytes) };
            return;
        }
        for index in written..written + match_bytes {
            unsafe { base.add(index).write(base.add(index - offset).read()) };
        }
    }

    unsafe fn copy_overlapping(
        base: *mut u8,
        size: usize,
        written: usize,
        offset: usize,
        match_bytes: usize,
    ) {
        assert!(
            offset > 0
                && offset <= written
                && offset < match_bytes
                && match_bytes.next_multiple_of(OVERLAP_CHUNK_BYTES) <= size - written,
            "the overlapping match and its chunks lie inside the output"
        );
        let mut copied = 0;
        let mut distance = offset;
        if offset < OVERLAP_CHUNK_BYTES {
            for (index, &pattern) in OVERLAP_PATTERN_INDICES[offset].iter().enumerate() {
                unsafe {
                    base.add(written + index)
                        .write(base.add(written - offset + pattern).read())
                };
            }
            copied = OVERLAP_CHUNK_BYTES;
            distance = OVERLAP_DISTANCES[offset];
        }
        while copied < match_bytes {
            unsafe {
                ptr::copy_nonoverlapping(
                    base.add(written + copied - distance),
                    base.add(written + copied),
                    OVERLAP_CHUNK_BYTES,
                );
            }
            copied += OVERLAP_CHUNK_BYTES;
        }
    }

    const fn overlap_pattern_indices() -> [[usize; OVERLAP_CHUNK_BYTES]; OVERLAP_CHUNK_BYTES] {
        let mut indices = [[0; OVERLAP_CHUNK_BYTES]; OVERLAP_CHUNK_BYTES];
        let mut offset = 1;
        while offset < OVERLAP_CHUNK_BYTES {
            let mut index = 0;
            while index < OVERLAP_CHUNK_BYTES {
                indices[offset][index] = index % offset;
                index += 1;
            }
            offset += 1;
        }
        indices
    }

    const fn overlap_distances() -> [usize; OVERLAP_CHUNK_BYTES] {
        let mut distances = [0; OVERLAP_CHUNK_BYTES];
        let mut offset = 1;
        while offset < OVERLAP_CHUNK_BYTES {
            distances[offset] = offset * OVERLAP_CHUNK_BYTES.div_ceil(offset);
            offset += 1;
        }
        distances
    }
}
