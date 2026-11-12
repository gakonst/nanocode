use crate::tables::{
    CANONICAL_DECOMPOSITION_CHARACTERS, CANONICAL_DECOMPOSITIONS, CANONICAL_UNMAPPED_STARTER_BELOW,
    COMBINING_CLASS_BLOCK_BITS, COMBINING_CLASS_BLOCKS, COMBINING_CLASSES,
    COMPATIBILITY_DECOMPOSITION_CHARACTERS, COMPATIBILITY_DECOMPOSITIONS,
    COMPATIBILITY_UNMAPPED_STARTER_BELOW, COMPOSITION_FIRSTS, COMPOSITION_SECONDS,
};

const HANGUL_SYLLABLE_FIRST: u32 = 0xAC00;
const HANGUL_LEADING_FIRST: u32 = 0x1100;
const HANGUL_VOWEL_FIRST: u32 = 0x1161;
const HANGUL_TRAILING_BASE: u32 = 0x11A7;
const HANGUL_LEADING_COUNT: u32 = 19;
const HANGUL_VOWEL_COUNT: u32 = 21;
const HANGUL_TRAILING_COUNT: u32 = 28;
const HANGUL_VOWEL_TRAILING_COUNT: u32 = HANGUL_VOWEL_COUNT * HANGUL_TRAILING_COUNT;
const HANGUL_SYLLABLE_COUNT: u32 = HANGUL_LEADING_COUNT * HANGUL_VOWEL_TRAILING_COUNT;
const COMBINING_CLASS_BLOCK_MASK: u32 = (1 << COMBINING_CLASS_BLOCK_BITS) - 1;

#[derive(Clone, Copy)]
pub(crate) enum Form {
    Canonical,
    Compatibility,
}

impl Form {
    pub(crate) const fn unmapped_starter_below(self) -> char {
        match self {
            Self::Canonical => CANONICAL_UNMAPPED_STARTER_BELOW,
            Self::Compatibility => COMPATIBILITY_UNMAPPED_STARTER_BELOW,
        }
    }
}

pub(crate) fn combining_class(character: char) -> u8 {
    let code = u32::from(character);
    let block = (code >> COMBINING_CLASS_BLOCK_BITS) as usize;
    match COMBINING_CLASS_BLOCKS.get(block) {
        None => 0,
        Some(&index) => {
            let offset = (usize::from(index) << COMBINING_CLASS_BLOCK_BITS)
                | (code & COMBINING_CLASS_BLOCK_MASK) as usize;
            COMBINING_CLASSES[offset]
        }
    }
}

fn mapping(
    table: &[(char, u16, u16)],
    characters: &'static [char],
    character: char,
) -> Option<&'static [char]> {
    let row = table
        .binary_search_by_key(&character, |entry| entry.0)
        .ok()?;
    let (_, start, end) = table[row];
    Some(&characters[usize::from(start)..usize::from(end)])
}

const fn jamo(code: u32) -> char {
    char::from_u32(code).expect("a Hangul jamo is a Unicode scalar value")
}

pub(crate) fn decompose(character: char, form: Form, mut emit: impl FnMut(char)) {
    let code = u32::from(character);
    if let Some(syllable) = code
        .checked_sub(HANGUL_SYLLABLE_FIRST)
        .filter(|syllable| *syllable < HANGUL_SYLLABLE_COUNT)
    {
        emit(jamo(
            HANGUL_LEADING_FIRST + syllable / HANGUL_VOWEL_TRAILING_COUNT,
        ));
        emit(jamo(
            HANGUL_VOWEL_FIRST + (syllable % HANGUL_VOWEL_TRAILING_COUNT) / HANGUL_TRAILING_COUNT,
        ));
        let trailing = syllable % HANGUL_TRAILING_COUNT;
        if trailing != 0 {
            emit(jamo(HANGUL_TRAILING_BASE + trailing));
        }
        return;
    }
    let canonical = || {
        mapping(
            &CANONICAL_DECOMPOSITIONS,
            &CANONICAL_DECOMPOSITION_CHARACTERS,
            character,
        )
    };
    let decomposition = match form {
        Form::Canonical => canonical(),
        Form::Compatibility => mapping(
            &COMPATIBILITY_DECOMPOSITIONS,
            &COMPATIBILITY_DECOMPOSITION_CHARACTERS,
            character,
        )
        .or_else(canonical),
    };
    match decomposition {
        Some(characters) => characters.iter().copied().for_each(emit),
        None => emit(character),
    }
}

fn compose_hangul(first: u32, second: u32) -> Option<char> {
    let leading = first.wrapping_sub(HANGUL_LEADING_FIRST);
    let vowel = second.wrapping_sub(HANGUL_VOWEL_FIRST);
    if leading < HANGUL_LEADING_COUNT && vowel < HANGUL_VOWEL_COUNT {
        return Some(jamo(
            HANGUL_SYLLABLE_FIRST
                + leading * HANGUL_VOWEL_TRAILING_COUNT
                + vowel * HANGUL_TRAILING_COUNT,
        ));
    }
    let syllable = first.wrapping_sub(HANGUL_SYLLABLE_FIRST);
    let trailing = second.wrapping_sub(HANGUL_TRAILING_BASE);
    let open = syllable < HANGUL_SYLLABLE_COUNT && syllable.is_multiple_of(HANGUL_TRAILING_COUNT);
    (open && trailing != 0 && trailing < HANGUL_TRAILING_COUNT).then(|| jamo(first + trailing))
}

pub(crate) fn compose(first: char, second: char) -> Option<char> {
    if let Some(syllable) = compose_hangul(u32::from(first), u32::from(second)) {
        return Some(syllable);
    }
    let row = COMPOSITION_SECONDS
        .binary_search_by_key(&second, |entry| entry.0)
        .ok()?;
    let (_, start, end) = COMPOSITION_SECONDS[row];
    let firsts = &COMPOSITION_FIRSTS[usize::from(start)..usize::from(end)];
    let column = firsts.binary_search_by_key(&first, |entry| entry.0).ok()?;
    Some(firsts[column].1)
}
