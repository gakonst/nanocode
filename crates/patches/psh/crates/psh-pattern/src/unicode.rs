use alloc::string::String;
use alloc::vec::Vec;

use crate::class::{ClassUnicode, Range, SCALAR_MAX};
use crate::unicode_data as data;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LookupError {
    PropertyNotFound,
    PropertyValueNotFound,
}

pub enum Query<'a> {
    OneLetter(char),
    Binary(&'a str),
    ByValue { name: &'a str, value: &'a str },
}

enum Canonical {
    Binary(&'static str),
    GeneralCategory(&'static str),
    Script(&'static str),
    ByValue {
        name: &'static str,
        value: &'static str,
    },
}

const ASCII_LAST: u32 = 0x7F;

pub fn class(query: Query<'_>) -> Result<ClassUnicode, LookupError> {
    match canonicalize(query)? {
        Canonical::Binary(name) => bool_property(name),
        Canonical::GeneralCategory(name) => general_category(name),
        Canonical::Script(name) => {
            named_set(data::SCRIPT, name, LookupError::PropertyValueNotFound)
        }
        Canonical::ByValue { name: "Age", value } => ages(value),
        Canonical::ByValue {
            name: "Script_Extensions",
            value,
        } => named_set(
            data::SCRIPT_EXTENSION,
            value,
            LookupError::PropertyValueNotFound,
        ),
        Canonical::ByValue {
            name: "Grapheme_Cluster_Break",
            value,
        } => named_set(
            data::GRAPHEME_CLUSTER_BREAK,
            value,
            LookupError::PropertyValueNotFound,
        ),
        Canonical::ByValue {
            name: "Sentence_Break",
            value,
        } => named_set(
            data::SENTENCE_BREAK,
            value,
            LookupError::PropertyValueNotFound,
        ),
        Canonical::ByValue {
            name: "Word_Break",
            value,
        } => named_set(data::WORD_BREAK, value, LookupError::PropertyValueNotFound),
        Canonical::ByValue { .. } => Err(LookupError::PropertyNotFound),
    }
}

fn canonicalize(query: Query<'_>) -> Result<Canonical, LookupError> {
    match query {
        Query::OneLetter(c) => {
            let mut buffer = [0; 4];
            canonical_binary(c.encode_utf8(&mut buffer))
        }
        Query::Binary(name) => canonical_binary(name),
        Query::ByValue { name, value } => {
            let name = normalize(name);
            let value = normalize(value);
            let canonical_name = canonical_property(&name).ok_or(LookupError::PropertyNotFound)?;
            match canonical_name {
                "General_Category" => {
                    let canonical = canonical_general_category(&value)
                        .ok_or(LookupError::PropertyValueNotFound)?;
                    Ok(Canonical::GeneralCategory(canonical))
                }
                "Script" => {
                    let canonical =
                        canonical_script(&value).ok_or(LookupError::PropertyValueNotFound)?;
                    Ok(Canonical::Script(canonical))
                }
                _ => {
                    let values = property_values(canonical_name)
                        .ok_or(LookupError::PropertyValueNotFound)?;
                    let canonical = canonical_value(values, &value)
                        .ok_or(LookupError::PropertyValueNotFound)?;
                    Ok(Canonical::ByValue {
                        name: canonical_name,
                        value: canonical,
                    })
                }
            }
        }
    }
}

fn canonical_binary(name: &str) -> Result<Canonical, LookupError> {
    let normalized = normalize(name);
    if normalized != "cf"
        && normalized != "sc"
        && normalized != "lc"
        && let Some(canonical) = canonical_property(&normalized)
    {
        return Ok(Canonical::Binary(canonical));
    }
    if let Some(canonical) = canonical_general_category(&normalized) {
        return Ok(Canonical::GeneralCategory(canonical));
    }
    if let Some(canonical) = canonical_script(&normalized) {
        return Ok(Canonical::Script(canonical));
    }
    Err(LookupError::PropertyNotFound)
}

fn canonical_general_category(normalized: &str) -> Option<&'static str> {
    match normalized {
        "any" => Some("Any"),
        "assigned" => Some("Assigned"),
        "ascii" => Some("ASCII"),
        _ => canonical_value(
            property_values("General_Category").expect("general categories have values"),
            normalized,
        ),
    }
}

fn canonical_script(normalized: &str) -> Option<&'static str> {
    canonical_value(
        property_values("Script").expect("scripts have values"),
        normalized,
    )
}

fn canonical_property(normalized: &str) -> Option<&'static str> {
    data::PROPERTY_NAMES
        .binary_search_by_key(&normalized, |&(name, _)| name)
        .ok()
        .map(|index| data::PROPERTY_NAMES[index].1)
}

fn property_values(canonical: &str) -> Option<&'static [(&'static str, &'static str)]> {
    data::PROPERTY_VALUES
        .binary_search_by_key(&canonical, |&(name, _)| name)
        .ok()
        .map(|index| data::PROPERTY_VALUES[index].1)
}

fn canonical_value(
    values: &'static [(&'static str, &'static str)],
    normalized: &str,
) -> Option<&'static str> {
    values
        .binary_search_by_key(&normalized, |&(name, _)| name)
        .ok()
        .map(|index| values[index].1)
}

fn named_set(
    table: &'static [(&'static str, &'static [(u32, u32)])],
    canonical: &str,
    missing: LookupError,
) -> Result<ClassUnicode, LookupError> {
    table
        .binary_search_by_key(&canonical, |&(name, _)| name)
        .ok()
        .map(|index| ClassUnicode::from_table(table[index].1))
        .ok_or(missing)
}

fn general_category(canonical: &str) -> Result<ClassUnicode, LookupError> {
    match canonical {
        "Decimal_Number" => Ok(perl_digit()),
        "ASCII" => Ok(ClassUnicode::new([Range {
            start: 0,
            end: ASCII_LAST,
        }])),
        "Any" => Ok(ClassUnicode::new([Range {
            start: 0,
            end: SCALAR_MAX,
        }])),
        "Assigned" => {
            let mut class = general_category("Unassigned")?;
            class.negate();
            Ok(class)
        }
        name => named_set(
            data::GENERAL_CATEGORY,
            name,
            LookupError::PropertyValueNotFound,
        ),
    }
}

fn bool_property(canonical: &str) -> Result<ClassUnicode, LookupError> {
    match canonical {
        "Decimal_Number" => Ok(perl_digit()),
        "White_Space" => Ok(perl_space()),
        name => named_set(data::PROPERTY_BOOL, name, LookupError::PropertyNotFound),
    }
}

type AgeTable = (u32, u32, &'static [(u32, u32)]);

fn ages(canonical: &str) -> Result<ClassUnicode, LookupError> {
    let mut versions: Vec<AgeTable> = data::AGE
        .iter()
        .map(|&(name, table)| {
            let (major, minor) = name[1..]
                .split_once('_')
                .expect("age names are V<major>_<minor>");
            (
                major.parse().expect("major version"),
                minor.parse().expect("minor version"),
                table,
            )
        })
        .collect();
    versions.sort_by_key(|&(major, minor, _)| (major, minor));
    let position = data::AGE
        .iter()
        .position(|&(name, _)| name == canonical)
        .ok_or(LookupError::PropertyValueNotFound)?;
    let (major, minor) = data::AGE[position].0[1..]
        .split_once('_')
        .expect("age names are V<major>_<minor>");
    let limit: (u32, u32) = (
        major.parse().expect("major version"),
        minor.parse().expect("minor version"),
    );
    let mut class = ClassUnicode::empty();
    for (major, minor, table) in versions {
        if (major, minor) <= limit {
            class.union(&ClassUnicode::from_table(table));
        }
    }
    Ok(class)
}

pub fn perl_word() -> ClassUnicode {
    ClassUnicode::from_table(data::PERL_WORD)
}

pub fn perl_digit() -> ClassUnicode {
    ClassUnicode::from_table(data::PERL_DECIMAL)
}

pub fn perl_space() -> ClassUnicode {
    ClassUnicode::from_table(data::PERL_SPACE)
}

pub fn is_word_character(c: char) -> bool {
    let value = u32::from(c);
    if value <= ASCII_LAST {
        let byte = value as u8;
        return byte.is_ascii_alphanumeric() || byte == b'_';
    }
    data::PERL_WORD
        .binary_search_by(|&(start, end)| {
            if end < value {
                core::cmp::Ordering::Less
            } else if start > value {
                core::cmp::Ordering::Greater
            } else {
                core::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

pub fn simple_fold_overlaps(start: u32, end: u32) -> bool {
    let table = data::CASE_FOLDING_SIMPLE;
    let first = table.partition_point(|&(c, _)| c < start);
    first < table.len() && table[first].0 <= end
}

pub fn simple_fold_range(start: u32, end: u32, out: &mut Vec<u32>) {
    let table = data::CASE_FOLDING_SIMPLE;
    let first = table.partition_point(|&(c, _)| c < start);
    for &(c, mapped) in &table[first..] {
        if c > end {
            break;
        }
        out.extend_from_slice(mapped);
    }
}

pub fn normalize(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut start = 0;
    let starts_with_is = bytes.len() >= 2 && bytes[..2].eq_ignore_ascii_case(b"is");
    if starts_with_is {
        start = 2;
    }
    let mut out = String::new();
    for &b in &bytes[start..] {
        if b == b' ' || b == b'_' || b == b'-' {
            continue;
        } else if b.is_ascii_uppercase() {
            out.push(char::from(b.to_ascii_lowercase()));
        } else if b.is_ascii() {
            out.push(char::from(b));
        }
    }
    if starts_with_is && out == "c" {
        out = String::from("isc");
    }
    out
}
