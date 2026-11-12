use super::properties::{
    CASE_FOLDS, PROPERTY_RANGES, TABLE_OFFSETS, binary_property, general_category, script,
};
use super::set::{CodePointSet, Interval};

const WORD_BYTES: u32 = 4;
const PAIR_BYTES: usize = 8;

pub(super) const WORD_CHARACTERS: [Interval; 4] = [
    Interval::new('0' as u32, '9' as u32),
    Interval::new('A' as u32, 'Z' as u32),
    Interval::new('_' as u32, '_' as u32),
    Interval::new('a' as u32, 'z' as u32),
];

pub(super) const DIGITS: [Interval; 1] = [Interval::new('0' as u32, '9' as u32)];

pub(super) const WHITE_SPACE: [Interval; 9] = [
    Interval::new(0x0009, 0x000C),
    Interval::new(0x0020, 0x0020),
    Interval::new(0x00A0, 0x00A0),
    Interval::new(0x1680, 0x1680),
    Interval::new(0x2000, 0x200A),
    Interval::new(0x202F, 0x202F),
    Interval::new(0x205F, 0x205F),
    Interval::new(0x3000, 0x3000),
    Interval::new(0xFEFF, 0xFEFF),
];

pub(super) const LINE_TERMINATORS: [Interval; 3] = [
    Interval::new(0x000A, 0x000A),
    Interval::new(0x000D, 0x000D),
    Interval::new(0x2028, 0x2029),
];

const LONG_S: u32 = 0x017F;
const KELVIN_SIGN: u32 = 0x212A;

#[derive(Clone, Copy, Debug)]
pub(super) enum PropertyName {
    GeneralCategory,
    Script,
    ScriptExtensions,
}

pub(super) fn property_name(name: &str) -> Option<PropertyName> {
    match name {
        "General_Category" | "gc" => Some(PropertyName::GeneralCategory),
        "Script" | "sc" => Some(PropertyName::Script),
        "Script_Extensions" | "scx" => Some(PropertyName::ScriptExtensions),
        _ => None,
    }
}

pub(super) fn property(value: &str, name: Option<PropertyName>) -> Option<CodePointSet> {
    let table = match name {
        Some(PropertyName::GeneralCategory) => general_category(value)?,
        Some(PropertyName::Script) => script(value)?.0,
        Some(PropertyName::ScriptExtensions) => script(value)?.1,
        None => binary_property(value).or_else(|| general_category(value))?,
    };
    Some(table_set(table))
}

fn read_word(bytes: &[u8], index: usize) -> u32 {
    let start = index * WORD_BYTES as usize;
    let word: [u8; WORD_BYTES as usize] = bytes[start..start + WORD_BYTES as usize]
        .try_into()
        .expect("embedded Unicode tables hold whole words");
    u32::from_le_bytes(word)
}

fn table_set(table: usize) -> CodePointSet {
    let first = TABLE_OFFSETS[table] as usize;
    let end = TABLE_OFFSETS[table + 1] as usize;
    let intervals: Vec<Interval> = (first..end)
        .map(|pair| {
            Interval::new(
                read_word(PROPERTY_RANGES, pair * 2),
                read_word(PROPERTY_RANGES, pair * 2 + 1),
            )
        })
        .collect();
    CodePointSet::from_sorted(intervals)
}

pub(super) fn is_identifier_start(code_point: u32) -> bool {
    table_contains(identifier_table("ID_Start"), code_point)
}

pub(super) fn is_identifier_continue(code_point: u32) -> bool {
    table_contains(identifier_table("ID_Continue"), code_point)
}

fn identifier_table(name: &str) -> usize {
    binary_property(name).expect("ID_Start and ID_Continue are binary properties")
}

fn table_contains(table: usize, code_point: u32) -> bool {
    let mut low = TABLE_OFFSETS[table] as usize;
    let mut high = TABLE_OFFSETS[table + 1] as usize;
    while low < high {
        let middle = low + (high - low) / 2;
        if read_word(PROPERTY_RANGES, middle * 2 + 1) < code_point {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low < TABLE_OFFSETS[table + 1] as usize && read_word(PROPERTY_RANGES, low * 2) <= code_point
}

fn fold_count() -> usize {
    assert_eq!(CASE_FOLDS.len() % PAIR_BYTES, 0);
    CASE_FOLDS.len() / PAIR_BYTES
}

fn fold_pair(index: usize) -> (u32, u32) {
    (
        read_word(CASE_FOLDS, index * 2),
        read_word(CASE_FOLDS, index * 2 + 1),
    )
}

pub(super) fn fold(code_point: u32) -> u32 {
    let mut low = 0;
    let mut high = fold_count();
    while low < high {
        let middle = low + (high - low) / 2;
        let (source, target) = fold_pair(middle);
        if source == code_point {
            return target;
        }
        if source < code_point {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    code_point
}

pub(super) fn unfold(code_point: u32) -> Vec<u32> {
    let folded = fold(code_point);
    let mut unfolded = vec![code_point, folded];
    for index in 0..fold_count() {
        let (source, target) = fold_pair(index);
        if target == folded {
            unfolded.push(source);
        }
    }
    unfolded.sort_unstable();
    unfolded.dedup();
    unfolded
}

pub(super) fn add_case_variants(set: &CodePointSet) -> CodePointSet {
    let mut folded = set.clone();
    for index in 0..fold_count() {
        let (source, target) = fold_pair(index);
        if set.contains(source) {
            folded.add_one(target);
        }
    }
    let mut result = folded.clone();
    for index in 0..fold_count() {
        let (source, target) = fold_pair(index);
        if folded.contains(target) {
            result.add_one(source);
        }
    }
    result
}

pub(super) const fn is_word_character(code_point: u32) -> bool {
    matches!(code_point, 0x30..=0x39 | 0x41..=0x5A | 0x5F | 0x61..=0x7A)
}

pub(super) const fn is_word_character_ignoring_case(code_point: u32) -> bool {
    is_word_character(code_point) || code_point == LONG_S || code_point == KELVIN_SIGN
}

pub(super) const fn is_line_terminator(code_point: u32) -> bool {
    matches!(code_point, 0x000A | 0x000D | 0x2028 | 0x2029)
}
