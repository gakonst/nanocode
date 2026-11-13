//! Access to the Unicode character database (`unicodedata`).
//!
//! Owns the generated Unicode 3.2.0 / latest tables.

// spell-checker:ignore codep decomp DECOMP unidata

use core::{cmp::Ordering, fmt::Write, hint::cold_path};

use alloc::{
    format,
    string::String,
};

use rustpython_wtf8::CodePoint;

use crate::properties::{self, BidiClass, EastAsianWidth, GeneralCategory, NumericType};

const COMBINING_CLASS_NOT_REORDERED: u8 = 0;
const HANGUL_SYLLABLE_FIRST: u32 = 0xAC00;
const HANGUL_LEADING_FIRST: u32 = 0x1100;
const HANGUL_VOWEL_FIRST: u32 = 0x1161;
const HANGUL_TRAILING_BASE: u32 = 0x11A7;
const HANGUL_LEADING_COUNT: u32 = 19;
const HANGUL_VOWEL_COUNT: u32 = 21;
const HANGUL_TRAILING_COUNT: u32 = 28;
const HANGUL_VOWEL_TRAILING_COUNT: u32 = HANGUL_VOWEL_COUNT * HANGUL_TRAILING_COUNT;
const HANGUL_SYLLABLE_COUNT: u32 = HANGUL_LEADING_COUNT * HANGUL_VOWEL_TRAILING_COUNT;

fn hangul_decomposition(code: u32) -> Option<(u32, u32)> {
    let syllable = code
        .checked_sub(HANGUL_SYLLABLE_FIRST)
        .filter(|syllable| *syllable < HANGUL_SYLLABLE_COUNT)?;
    let trailing = syllable % HANGUL_TRAILING_COUNT;
    if trailing == 0 {
        let leading = HANGUL_LEADING_FIRST + syllable / HANGUL_VOWEL_TRAILING_COUNT;
        let vowel =
            HANGUL_VOWEL_FIRST + (syllable % HANGUL_VOWEL_TRAILING_COUNT) / HANGUL_TRAILING_COUNT;
        Some((leading, vowel))
    } else {
        Some((code - trailing, HANGUL_TRAILING_BASE + trailing))
    }
}

include!(concat!(env!("OUT_DIR"), "/generated/algo_names.rs"));
include!(concat!(env!("OUT_DIR"), "/generated/name_lookups.rs"));
include!(concat!(env!("OUT_DIR"), "/generated/bidi_class_3_2.rs"));
include!(concat!(env!("OUT_DIR"), "/generated/binary_props_3_2.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/generated/combining_class_3_2.rs"
));
include!(concat!(env!("OUT_DIR"), "/generated/decomp.rs"));
include!(concat!(env!("OUT_DIR"), "/generated/eaw_3_2.rs"));
include!(concat!(env!("OUT_DIR"), "/generated/gen_cat_3_2.rs"));
include!(concat!(env!("OUT_DIR"), "/generated/membership_3_2.rs"));
include!(concat!(env!("OUT_DIR"), "/generated/numeric_value_3_2.rs"));
include!(concat!(env!("OUT_DIR"), "/generated/num_type_3_2.rs"));

#[derive(Clone, Copy)]
enum DecompositionType {
    Canonical,
    Compat,
    Circle,
    Final,
    Font,
    Fraction,
    Initial,
    Isolated,
    Medial,
    Narrow,
    Nobreak,
    Small,
    Square,
    Sub,
    Super,
    Vertical,
    Wide,
}

impl DecompositionType {
    const fn type_tag(self) -> Option<&'static str> {
        match self {
            Self::Canonical => None,
            Self::Compat => Some("compat"),
            Self::Circle => Some("circle"),
            Self::Final => Some("final"),
            Self::Font => Some("font"),
            Self::Fraction => Some("fraction"),
            Self::Initial => Some("initial"),
            Self::Isolated => Some("isolated"),
            Self::Medial => Some("medial"),
            Self::Narrow => Some("narrow"),
            Self::Nobreak => Some("noBreak"),
            Self::Small => Some("small"),
            Self::Square => Some("square"),
            Self::Sub => Some("sub"),
            Self::Super => Some("super"),
            Self::Vertical => Some("vertical"),
            Self::Wide => Some("wide"),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum AlgorithmicName {
    TangutIdeograph,
}

impl AlgorithmicName {
    const fn name_base(self) -> &'static str {
        match self {
            Self::TangutIdeograph => "TANGUT IDEOGRAPH",
        }
    }

    fn to_name(self, ch: char) -> String {
        match self {
            Self::TangutIdeograph => format!("{}-{:0X}", self.name_base(), ch as u32),
        }
    }

    fn check_name(search_name: &str) -> Option<char> {
        // CPython compares derived-name prefixes case-insensitively
        // (`PyOS_strnicmp` in `_getcode`).
        let folded: String = search_name
            .chars()
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if let Some(without_base) = folded.strip_prefix(Self::TangutIdeograph.name_base()) {
            // `parse_hex_code` does not skip spaces around the digits.
            let hex = without_base.strip_prefix('-')?;
            // `_getcode` / `parse_hex_code`: 4–6 digits, no leading zero.
            if hex.len() < 4 || hex.len() > 6 || hex.starts_with('0') {
                return None;
            }
            let cp = u32::from_str_radix(hex, 16).ok()?;
            let ch = char::from_u32(cp)?;
            if lookup_table(ALGO_NAMES, ch)? == Self::TangutIdeograph {
                return Some(ch);
            }
        }

        None
    }
}

/// Result of `unicodedata.lookup`.
///
/// A named sequence is several characters; `\N{...}` only accepts a
/// single character, so `lookup_character` drops `Sequence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupResult {
    Character(char),
    Sequence(&'static str),
}

fn fold_lookup_name(name: &str) -> String {
    name.chars().map(|c| c.to_ascii_uppercase()).collect()
}

fn lookup_alias(name: &str) -> Option<char> {
    let key = fold_lookup_name(name);
    NAME_ALIASES
        .binary_search_by_key(&key.as_str(), |(n, _)| *n)
        .ok()
        .map(|i| NAME_ALIASES[i].1)
}

fn lookup_named_sequence(name: &str) -> Option<&'static str> {
    let key = fold_lookup_name(name);
    NAMED_SEQUENCES
        .binary_search_by_key(&key.as_str(), |(n, _)| *n)
        .ok()
        .map(|i| NAMED_SEQUENCES[i].1)
}

fn lookup_table<T: Copy>(table: &[(u32, u32, T)], ch: char) -> Option<T> {
    let ch = ch as u32;
    table
        .binary_search_by(|&(start, end, _)| {
            if ch > end {
                Ordering::Less
            } else if ch < start {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .ok()
        .map(|i| table[i].2)
}

#[cold]
#[inline(never)]
#[must_use]
pub fn membership_3_2(ch: u32) -> bool {
    MEMBERSHIP_3_2
        .binary_search_by(|&(start, end)| {
            if ch > end {
                Ordering::Less
            } else if ch < start {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

fn lookup_property_diff<T: Copy>(
    table: &[(u32, u32, T)],
    diff: &[(u32, u32, T)],
    ch: char,
    modern: bool,
) -> Option<T> {
    if modern {
        lookup_table(table, ch)
    } else {
        cold_path();
        lookup_table(diff, ch).or_else(|| {
            membership_3_2(ch as u32)
                .then(|| lookup_table(table, ch))
                .flatten()
        })
    }
}

/// The version string of the latest Unicode database bundled with the standard
/// library (`unicodedata.unidata_version`).
#[must_use]
pub fn unicode_version() -> String {
    format!(
        "{}.{}.{}",
        char::UNICODE_VERSION.0,
        char::UNICODE_VERSION.1,
        char::UNICODE_VERSION.2
    )
}

/// Look up a name in the latest UCD (`unicodedata.lookup`).
///
/// Named sequences are included; `\N{...}` should use [`lookup_character`]
/// so a sequence is reported as unknown.
#[must_use]
pub fn lookup_name(search_name: &str) -> Option<LookupResult> {
    Ucd::new(true).lookup(search_name)
}

/// Look up a single character by its Unicode name (`\N{...}`).
///
/// Named sequences are not characters and so do not resolve.
#[must_use]
pub fn lookup_character(search_name: &str) -> Option<char> {
    match lookup_name(search_name) {
        Some(LookupResult::Character(ch)) => Some(ch),
        Some(LookupResult::Sequence(_)) | None => None,
    }
}

/// The Unicode name of `ch` (`unicodedata.name`), if any.
#[must_use]
pub fn character_name(ch: char) -> Option<String> {
    crate::names::name(ch)
        .or_else(|| lookup_table(ALGO_NAMES, ch).map(|v| v.to_name(ch)))
}

/// A view over the Unicode character database at a fixed version.
///
/// `modern` selects the latest bundled UCD; otherwise the Unicode 3.2.0 tables
/// used by `unicodedata.ucd_3_2_0` are consulted.
#[derive(Debug, Clone, Copy)]
pub struct Ucd {
    modern: bool,
}

impl Ucd {
    #[must_use]
    pub const fn new(modern: bool) -> Self {
        Self { modern }
    }

    #[must_use]
    pub fn membership(self, ch: char) -> bool {
        if !self.modern {
            cold_path();
            membership_3_2(ch as u32)
        } else {
            // Rust chars always "exist" for modern Unicode or else they wouldn't be chars.
            true
        }
    }

    /// Look up `name` in this UCD view.
    ///
    /// Unicode 3.2.0 has no name aliases and no named sequences
    /// (`unicodedata.ucd_3_2_0.lookup`).
    #[must_use]
    pub fn lookup(self, search_name: &str) -> Option<LookupResult> {
        if let Some(ch) = crate::names::character(search_name)
            .or_else(|| AlgorithmicName::check_name(search_name))
        {
            return self.membership(ch).then_some(LookupResult::Character(ch));
        }
        if !self.modern {
            return None;
        }
        if let Some(ch) = lookup_alias(search_name) {
            return Some(LookupResult::Character(ch));
        }
        lookup_named_sequence(search_name).map(LookupResult::Sequence)
    }

    #[must_use]
    pub fn category(&self, c: CodePoint) -> &'static str {
        let Some(c) = c.to_char() else {
            return GeneralCategory::Surrogate.short_name();
        };
        if self.modern {
            Some(properties::general_category(c))
        } else {
            cold_path();
            lookup_table(GENERAL_CATEGORY, c)
        }
        .unwrap_or(GeneralCategory::Unassigned)
        .short_name()
    }

    #[must_use]
    pub fn bidirectional(&self, c: CodePoint) -> &'static str {
        c.to_char()
            .and_then(|c| {
                if self.modern {
                    Some(properties::bidi_class(c))
                } else {
                    cold_path();
                    lookup_table(BIDI_CLASS_DIFF, c)
                        .or_else(|| membership_3_2(c as u32).then(|| properties::bidi_class(c)))
                }
            })
            .map_or("", BidiClass::short_name)
    }

    #[must_use]
    pub fn east_asian_width(&self, c: CodePoint) -> &'static str {
        c.to_char()
            .and_then(|c| {
                if self.modern {
                    Some(properties::east_asian_width(c))
                } else {
                    cold_path();
                    // CPython overrides characters in the PUA for 3.2.0.
                    // Basic Multilingual Plane:
                    // https://en.wikipedia.org/wiki/Plane_(Unicode)#Basic_Multilingual_Plane
                    // https://en.wikipedia.org/wiki/Private_Use_Areas
                    // https://www.unicode.org/reports/tr11/tr11-10.html
                    // https://www.unicode.org/reports/tr11/
                    //
                    // Currently, this implementation is incomplete because I can't figure
                    // out what CPython is doing.
                    lookup_table(EAST_ASIAN_WIDTH, c)
                }
            })
            .unwrap_or(EastAsianWidth::Neutral)
            .short_name()
    }

    #[must_use]
    pub fn mirrored(&self, c: CodePoint) -> i32 {
        c.to_char().map_or(0, |c| {
            (if self.modern {
                properties::has(c, properties::BIDI_MIRRORED)
            } else {
                cold_path();
                let c = c as u32;
                BIDI_MIRRORED
                    .binary_search_by(|&(start, end)| {
                        if c > end {
                            Ordering::Less
                        } else if c < start {
                            Ordering::Greater
                        } else {
                            Ordering::Equal
                        }
                    })
                    .is_ok()
            }) as i32
        })
    }

    #[must_use]
    pub fn combining(&self, c: CodePoint) -> u8 {
        c.to_char()
            .and_then(|c| {
                if self.modern {
                    Some(unicode_normalization::combining_class(c))
                } else {
                    cold_path();
                    lookup_table(COMBINING_CLASS, c)
                }
            })
            .unwrap_or(COMBINING_CLASS_NOT_REORDERED)
    }

    #[must_use]
    pub fn decomposition(&self, c: CodePoint) -> String {
        let Some(ch) = c.to_char() else {
            return String::new();
        };

        // Decomposition is remarkable stable according to the normalization file,
        // so the updates slice is very small - only about four char pairs. Linearly searching
        // it is very fast. The file lists the original, incorrect decomp and the fixed char.
        // For 3.2.0, we use the original decomp for compatibility while ignoring the update.
        //
        // Finally, we don't have to do anything for the latest UCD as it's already updated.
        if self.modern
            && let Some((_, original)) = DECOMP_UPDATES
                .iter()
                .find(|&&(codep, _original)| codep == ch as u32)
        {
            format!("{original:04X}")
        } else if let Ok(i) =
            DECOMPOSITIONS.binary_search_by_key(&(ch as u32), |&(codep, _, _)| codep)
        {
            let (_, kind, end) = DECOMPOSITIONS[i];
            let start = i
                .checked_sub(1)
                .map(|i| DECOMPOSITIONS[i].2)
                .unwrap_or_default();

            let mut out = String::new();
            if let Some(tag) = kind.type_tag() {
                write!(out, "<{tag}>").unwrap();
            }
            for ch in &DECOMP_RANGE[start..end] {
                if !out.is_empty() {
                    out.push(' ');
                }
                write!(out, "{ch:04X}").unwrap();
            }

            out
        } else if let Some((first, second)) = hangul_decomposition(ch as u32) {
            format!("{first:04X} {second:04X}")
        } else {
            String::new()
        }
    }

    fn numeric_type_matches(self, ch: CodePoint, expected: &[NumericType]) -> Option<char> {
        let ch = ch.to_char()?;

        let actual = if self.modern {
            properties::numeric_type(ch)
        } else {
            cold_path();
            lookup_table(NUMERIC_TYPE_DIFF, ch).unwrap_or_else(|| properties::numeric_type(ch))
        };

        expected.contains(&actual).then_some(ch)
    }

    /// The integer digit value of `c` (`unicodedata.digit`), if it has one.
    #[must_use]
    pub fn digit(&self, c: CodePoint) -> Option<u64> {
        let expected = [NumericType::Decimal, NumericType::Digit];
        self.numeric_type_matches(c, &expected).and_then(|ch| {
            let value = lookup_property_diff(NUMERIC_VALUES, &[], ch, true)?;
            let int = value as u64;
            (int as f64 == value).then_some(int)
        })
    }

    /// The integer decimal value of `c` (`unicodedata.decimal`), if it has one.
    #[must_use]
    pub fn decimal(&self, c: CodePoint) -> Option<u64> {
        let expected = [NumericType::Decimal];
        self.numeric_type_matches(c, &expected).and_then(|ch| {
            let value = lookup_property_diff(NUMERIC_VALUES, NUMERIC_VALUES_DIFF, ch, self.modern)?;
            let int = value as u64;
            (int as f64 == value).then_some(int)
        })
    }

    /// The numeric value of `c` (`unicodedata.numeric`), if it has one.
    #[must_use]
    pub fn numeric(&self, c: CodePoint) -> Option<f64> {
        let expected = &[
            NumericType::Decimal,
            NumericType::Digit,
            NumericType::Numeric,
        ];
        self.numeric_type_matches(c, expected).and_then(|ch| {
            lookup_property_diff(NUMERIC_VALUES, NUMERIC_VALUES_DIFF, ch, self.modern)
        })
    }

    #[must_use]
    pub fn unidata_version(&self) -> String {
        if self.modern {
            unicode_version()
        } else {
            "3.2.0".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use rustpython_wtf8::CodePoint;

    use super::{LookupResult, Ucd, character_name, lookup_character, lookup_name};

    fn cp(ch: char) -> CodePoint {
        CodePoint::from(ch)
    }

    #[test]
    fn data_queries_match_unicodedata_behavior() {
        let ucd = Ucd::new(true);
        assert_eq!(ucd.category(cp('A')), "Lu");
        assert_eq!(ucd.category(CodePoint::from_u32(0xD800).unwrap()), "Cs");
        assert_eq!(lookup_character("SNOWMAN"), Some('☃'));
        assert_eq!(character_name('☃').as_deref(), Some("SNOWMAN"));
        assert_eq!(ucd.decimal(cp('५')), Some(5));
        assert_eq!(ucd.digit(cp('²')), Some(2));
        assert_eq!(ucd.numeric(cp('⅓')), Some(1.0 / 3.0));
        assert_eq!(ucd.numeric(cp('⅐')), Some(1.0 / 7.0));
    }

    #[test]
    fn ucd_3_2_0_view_differs_from_modern() {
        let legacy = Ucd::new(false);
        assert_eq!(legacy.unidata_version(), "3.2.0");
    }

    #[test]
    fn name_aliases_resolve_and_do_not_replace_the_official_name() {
        assert_eq!(
            lookup_character("LATIN CAPITAL LETTER GHA"),
            Some('\u{01A2}')
        );
        assert_eq!(
            lookup_character("latin capital letter gha"),
            Some('\u{01A2}')
        );
        assert_eq!(
            character_name('\u{01A2}').as_deref(),
            Some("LATIN CAPITAL LETTER OI")
        );
        assert_eq!(Ucd::new(false).lookup("LATIN CAPITAL LETTER GHA"), None);
    }

    #[test]
    fn named_sequences_resolve_only_on_the_modern_view() {
        assert_eq!(
            lookup_name("LATIN SMALL LETTER R WITH TILDE"),
            Some(LookupResult::Sequence("r\u{0303}"))
        );
        assert_eq!(
            lookup_name("KEYCAP NUMBER SIGN"),
            Some(LookupResult::Sequence("#\u{FE0F}\u{20E3}"))
        );
        assert_eq!(lookup_character("LATIN SMALL LETTER R WITH TILDE"), None);
        assert_eq!(
            Ucd::new(false).lookup("LATIN SMALL LETTER R WITH TILDE"),
            None
        );
    }

    #[test]
    fn tangut_derived_names_are_case_insensitive() {
        assert_eq!(
            lookup_character("TANGUT IDEOGRAPH-17000"),
            Some('\u{17000}')
        );
        assert_eq!(
            lookup_character("tangut ideograph-18d08"),
            Some('\u{18D08}')
        );
        assert_eq!(
            character_name('\u{17000}').as_deref(),
            Some("TANGUT IDEOGRAPH-17000")
        );
        assert_eq!(lookup_character("TANGUT IDEOGRAPH- 17000"), None);
        assert_eq!(lookup_character("TANGUT IDEOGRAPH-17000 "), None);
    }
}
