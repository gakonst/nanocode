mod tables;

use std::ops::RangeInclusive;
use tables::{LEAF_BITS, LEAVES, MIDDLE, MIDDLE_BITS, PROPERTIES, ROOT};

const LEAF_MASK: usize = (1 << LEAF_BITS) - 1;
const MIDDLE_MASK: usize = (1 << MIDDLE_BITS) - 1;
const LAST_NARROW_IN_SEQUENCE: char = '\u{A0}';
const ZERO_WIDTH_JOINER: char = '\u{200D}';
const QUOTE_SELECTOR: char = '\u{FE01}';
const TEXT_SELECTOR: char = '\u{FE0E}';
const EMOJI_SELECTOR: char = '\u{FE0F}';
const ARABIC_LETTER_LAM: char = '\u{644}';
const ARABIC_LETTERS_LAM_WITH_MARKS: RangeInclusive<char> = '\u{6B5}'..='\u{6B8}';
const ARABIC_LETTER_LAM_WITH_BAR: char = '\u{76A}';
const ARABIC_LETTER_LAM_WITH_SMALL_ARABIC_LETTER_TAH_ABOVE: char = '\u{8A6}';
const ARABIC_LETTER_LAM_WITH_SMALL_ARABIC_LETTER_JEEM_ABOVE: char = '\u{8C7}';
const HEBREW_LETTER_ALEF: char = '\u{5D0}';
const KHMER_SIGN_COENG: char = '\u{17D2}';
const BUGINESE_LETTER_A: char = '\u{1A15}';
const BUGINESE_VOWEL_SIGN_I: char = '\u{1A17}';
const TIFINAGH_CONSONANTS: RangeInclusive<char> = '\u{2D31}'..='\u{2D65}';
const TIFINAGH_LETTER_YAWA: char = '\u{2D6F}';
const TIFINAGH_CONSONANT_JOINER: char = '\u{2D7F}';
const LISU_LETTER_TONES: RangeInclusive<char> = '\u{A4F8}'..='\u{A4FB}';
const OLD_TURKIC_LETTER_ORKHON_EC: char = '\u{10C32}';
const KIRAT_RAI_SIGN_VIRAMA: char = '\u{16D63}';
const KIRAT_RAI_VOWEL_SIGN_E: char = '\u{16D67}';
const KIRAT_RAI_VOWEL_SIGN_AI: char = '\u{16D68}';
const KIRAT_RAI_VOWEL_SIGN_O: char = '\u{16D69}';
const LEFT_SINGLE_QUOTATION_MARK: char = '\u{2018}';
const RIGHT_SINGLE_QUOTATION_MARK: char = '\u{2019}';
const LEFT_DOUBLE_QUOTATION_MARK: char = '\u{201C}';
const RIGHT_DOUBLE_QUOTATION_MARK: char = '\u{201D}';
const COMBINING_ENCLOSING_KEYCAP: char = '\u{20E3}';
const REGIONAL_INDICATORS: RangeInclusive<char> = '\u{1F1E6}'..='\u{1F1FF}';
const EMOJI_MODIFIERS: RangeInclusive<char> = '\u{1F3FB}'..='\u{1F3FF}';
const WAVING_BLACK_FLAG: char = '\u{1F3F4}';
const CANCEL_TAG: char = '\u{E007F}';
const TAG_LETTERS: RangeInclusive<char> = '\u{E0061}'..='\u{E007A}';
const TAG_DIGITS: RangeInclusive<char> = '\u{E0030}'..='\u{E0039}';

#[derive(Clone, Copy, PartialEq, Eq)]
enum WidthKind {
    Zero,
    One,
    Two,
    Three,
    LineFeed,
    HebrewLetterLamed,
    JoiningGroupAlef,
    KhmerCoengEligibleLetter,
    BugineseLetterYa,
    TifinaghConsonant,
    LisuToneLetterMyaNaJeu,
    QuoteSelector,
    TextSelector,
    EmojiSelector,
    OldTurkicLetterOrkhonI,
    KiratRaiVowelSignE,
    KiratRaiVowelSignAi,
    RegionalIndicator,
    EmojiModifier,
    EmojiPresentation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GraphemeCategory {
    Other,
    CarriageReturn,
    LineFeed,
    Control,
    Extend,
    ZeroWidthJoiner,
    RegionalIndicator,
    Prepend,
    SpacingMark,
    LeadingJamo,
    VowelJamo,
    TrailingJamo,
    LeadingVowelSyllable,
    LeadingVowelTrailingSyllable,
    ExtendedPictographic,
    ConjunctConsonant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Conjunct {
    None,
    Linker,
    Extend,
}

struct Properties {
    width: WidthKind,
    grapheme: GraphemeCategory,
    conjunct: Conjunct,
    emoji_presentation_base: bool,
    narrow_text_presentation_base: bool,
    emoji_modifier_base: bool,
    joining_transparent: bool,
    ligature_transparent: bool,
}

#[inline]
fn properties(character: char) -> &'static Properties {
    let code_point = character as usize;
    let middle = usize::from(ROOT[code_point >> (LEAF_BITS + MIDDLE_BITS)]);
    let leaf = usize::from(MIDDLE[middle][(code_point >> LEAF_BITS) & MIDDLE_MASK]);
    &PROPERTIES[usize::from(LEAVES[leaf][code_point & LEAF_MASK])]
}

pub fn char_width(character: char) -> Option<usize> {
    if character.is_control() {
        return None;
    }
    if character.is_ascii() {
        return Some(1);
    }
    let (columns, _) = properties(character).width.alone();
    Some(usize::try_from(columns).expect("a character alone has a non-negative width"))
}

pub fn width(text: &str) -> usize {
    if text.is_ascii() {
        return ascii_width(text.as_bytes(), Following::Plain);
    }
    let mut characters = text.chars();
    let mut total = 0_usize;
    let mut following = Following::Plain;
    while let Some(character) = characters.next_back() {
        if character.is_ascii() && !following.emoji_selected() {
            let before = characters.as_str().as_bytes();
            let start = before
                .iter()
                .rposition(|byte| !byte.is_ascii())
                .map_or(0, |index| index + 1);
            let run = &text.as_bytes()[start..=before.len()];
            total += ascii_width(run, following);
            following = if run[0] == b'\n' {
                Following::LineFeed
            } else {
                Following::Plain
            };
            characters = text[..start].chars();
            continue;
        }
        let (columns, next) = step(character, following);
        total = total
            .checked_add_signed(columns)
            .expect("a negative width cancels a width already counted");
        following = next;
    }
    total
}

fn ascii_width(run: &[u8], following: Following) -> usize {
    let joined = memchr::memchr_iter(b'\r', run)
        .filter(|&index| run.get(index + 1) == Some(&b'\n'))
        .count();
    let joined_last = run.last() == Some(&b'\r') && following == Following::LineFeed;
    run.len() - joined - usize::from(joined_last)
}

impl WidthKind {
    #[inline]
    const fn alone(self) -> (isize, Following) {
        match self {
            Self::Zero => (0, Following::Plain),
            Self::One => (1, Following::Plain),
            Self::Two => (2, Following::Plain),
            Self::Three => (3, Following::Plain),
            Self::LineFeed => (1, Following::LineFeed),
            Self::HebrewLetterLamed => (1, Following::ligature(Ligature::HebrewLetterLamed)),
            Self::JoiningGroupAlef => (1, Following::ligature(Ligature::JoiningGroupAlef)),
            Self::KhmerCoengEligibleLetter => {
                (1, Following::ligature(Ligature::KhmerCoengEligibleLetter))
            }
            Self::BugineseLetterYa => (1, Following::ligature(Ligature::BugineseLetterYa)),
            Self::TifinaghConsonant => (1, Following::ligature(Ligature::TifinaghConsonant)),
            Self::LisuToneLetterMyaNaJeu => {
                (1, Following::ligature(Ligature::LisuToneLetterMyaNaJeu))
            }
            Self::QuoteSelector => (0, Following::QuoteSelector),
            Self::TextSelector => (0, Following::TextSelector),
            Self::EmojiSelector => (0, Following::EmojiSelector),
            Self::OldTurkicLetterOrkhonI => {
                (1, Following::ligature(Ligature::OldTurkicLetterOrkhonI))
            }
            Self::KiratRaiVowelSignE => (1, Following::KiratRaiVowelSignE),
            Self::KiratRaiVowelSignAi => (1, Following::KiratRaiVowelSignAi),
            Self::RegionalIndicator => (1, Following::RegionalIndicator),
            Self::EmojiModifier => (2, Following::EmojiModifier),
            Self::EmojiPresentation => (2, Following::EmojiPresentation),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ligature {
    JoiningGroupAlef,
    HebrewLetterLamed,
    JoinedHebrewLetterLamed,
    BugineseLetterYa,
    JoinedBugineseLetterYa,
    BugineseVowelSignIJoinedLetterYa,
    TifinaghConsonant,
    JoinedTifinaghConsonant,
    TifinaghJoinerConsonant,
    LisuToneLetterMyaNaJeu,
    OldTurkicLetterOrkhonI,
    JoinedOldTurkicLetterOrkhonI,
    KhmerCoengEligibleLetter,
}

impl Ligature {
    const fn joined(self) -> Self {
        match self {
            Self::HebrewLetterLamed => Self::JoinedHebrewLetterLamed,
            Self::BugineseLetterYa => Self::JoinedBugineseLetterYa,
            Self::TifinaghConsonant => Self::JoinedTifinaghConsonant,
            Self::OldTurkicLetterOrkhonI => Self::JoinedOldTurkicLetterOrkhonI,
            other => other,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Selectors {
    emoji: bool,
    text: bool,
    quote: bool,
}

impl Selectors {
    const NONE: Self = Self {
        emoji: false,
        text: false,
        quote: false,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TagRun {
    End,
    OneLetter,
    TwoLetters,
    ThreeLetters,
    FourLetters,
    FiveLetters,
    SixLetters,
    OneDigit,
    TwoDigits,
    ThreeDigits,
}

impl TagRun {
    const fn letter(self) -> Option<Self> {
        match self {
            Self::End => Some(Self::OneLetter),
            Self::OneLetter => Some(Self::TwoLetters),
            Self::TwoLetters => Some(Self::ThreeLetters),
            Self::ThreeLetters => Some(Self::FourLetters),
            Self::FourLetters => Some(Self::FiveLetters),
            Self::FiveLetters => Some(Self::SixLetters),
            Self::SixLetters | Self::OneDigit | Self::TwoDigits | Self::ThreeDigits => None,
        }
    }

    const fn digit(self) -> Option<Self> {
        match self {
            Self::End
            | Self::OneLetter
            | Self::TwoLetters
            | Self::ThreeLetters
            | Self::FourLetters => Some(Self::OneDigit),
            Self::OneDigit => Some(Self::TwoDigits),
            Self::TwoDigits => Some(Self::ThreeDigits),
            Self::FiveLetters | Self::SixLetters | Self::ThreeDigits => None,
        }
    }

    const fn completes_flag(self) -> bool {
        matches!(
            self,
            Self::ThreeLetters
                | Self::FourLetters
                | Self::FiveLetters
                | Self::SixLetters
                | Self::ThreeDigits
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Following {
    Plain,
    LineFeed,
    EmojiModifier,
    RegionalIndicator,
    RegionalIndicators,
    EmojiPresentation,
    JoinedEmoji,
    JoinedKeycap,
    SelectedJoinedEmoji,
    JoinedRegionalIndicator,
    EvenJoinedRegionalIndicators,
    OddJoinedRegionalIndicators,
    Tag(TagRun),
    KiratRaiVowelSignE,
    KiratRaiVowelSignAi,
    EmojiSelector,
    TextSelector,
    QuoteSelector,
    Ligature(Ligature, Selectors),
}

impl Following {
    const fn ligature(ligature: Ligature) -> Self {
        Self::Ligature(ligature, Selectors::NONE)
    }

    const fn emoji_selected(self) -> bool {
        match self {
            Self::EmojiSelector | Self::SelectedJoinedEmoji => true,
            Self::Ligature(_, selectors) => selectors.emoji,
            _ => false,
        }
    }

    const fn text_selected(self) -> bool {
        match self {
            Self::TextSelector => true,
            Self::Ligature(_, selectors) => selectors.text,
            _ => false,
        }
    }

    const fn quote_selected(self) -> bool {
        match self {
            Self::QuoteSelector => true,
            Self::Ligature(_, selectors) => selectors.quote,
            _ => false,
        }
    }

    const fn select_emoji(self) -> Self {
        match self {
            Self::Ligature(ligature, selectors) => Self::Ligature(
                ligature,
                Selectors {
                    emoji: true,
                    ..selectors
                },
            ),
            Self::JoinedEmoji | Self::JoinedKeycap => Self::SelectedJoinedEmoji,
            _ => Self::EmojiSelector,
        }
    }

    const fn select_text(self) -> Self {
        match self {
            Self::Ligature(ligature, selectors) => Self::Ligature(
                ligature,
                Selectors {
                    text: true,
                    ..selectors
                },
            ),
            _ => Self::TextSelector,
        }
    }

    const fn select_quote(self) -> Self {
        match self {
            Self::Ligature(ligature, selectors) => Self::Ligature(
                ligature,
                Selectors {
                    quote: true,
                    ..selectors
                },
            ),
            _ => Self::QuoteSelector,
        }
    }

    const fn deselect_emoji(self) -> Self {
        match self {
            Self::Ligature(ligature, selectors) => Self::Ligature(
                ligature,
                Selectors {
                    emoji: false,
                    ..selectors
                },
            ),
            _ => Self::Plain,
        }
    }

    const fn deselect_text(self) -> Self {
        match self {
            Self::Ligature(ligature, selectors) => Self::Ligature(
                ligature,
                Selectors {
                    text: false,
                    ..selectors
                },
            ),
            Self::TextSelector => Self::Plain,
            other => other,
        }
    }

    const fn deselect_quote(self) -> Self {
        match self {
            Self::Ligature(ligature, selectors) => Self::Ligature(
                ligature,
                Selectors {
                    quote: false,
                    ..selectors
                },
            ),
            Self::QuoteSelector => Self::Plain,
            other => other,
        }
    }
}

#[inline]
fn step(character: char, following: Following) -> (isize, Following) {
    if following == Following::Plain && character > LAST_NARROW_IN_SEQUENCE {
        return properties(character).width.alone();
    }
    let mut following = following;
    if following.emoji_selected() {
        if properties(character).emoji_presentation_base {
            let columns = if following == Following::SelectedJoinedEmoji {
                0
            } else {
                2
            };
            return (columns, Following::EmojiPresentation);
        }
        following = following.deselect_emoji();
    }
    if character <= LAST_NARROW_IN_SEQUENCE {
        return match character {
            '\n' => (1, Following::LineFeed),
            '\r' if following == Following::LineFeed => (0, Following::Plain),
            _ => (1, Following::Plain),
        };
    }
    let properties = properties(character);
    if following != Following::Plain
        && let Some(result) = sequence_step(character, properties, following)
    {
        return result;
    }
    properties.width.alone()
}

#[inline]
fn sequence_step(
    character: char,
    properties: &Properties,
    following: Following,
) -> Option<(isize, Following)> {
    match character {
        EMOJI_SELECTOR => return Some((0, following.select_emoji())),
        QUOTE_SELECTOR => return Some((0, following.select_quote())),
        TEXT_SELECTOR => return Some((0, following.select_text())),
        _ => {}
    }
    let mut following = following;
    if following.text_selected() {
        if properties.narrow_text_presentation_base {
            return Some((1, Following::Plain));
        }
        following = following.deselect_text();
    } else if following.quote_selected() {
        if matches!(
            character,
            LEFT_SINGLE_QUOTATION_MARK
                | RIGHT_SINGLE_QUOTATION_MARK
                | LEFT_DOUBLE_QUOTATION_MARK
                | RIGHT_DOUBLE_QUOTATION_MARK
        ) {
            return Some((2, Following::Plain));
        }
        following = following.deselect_quote();
    }
    match following {
        Following::Ligature(ligature, selectors) => {
            if ligature != Ligature::JoiningGroupAlef {
                if character == ZERO_WIDTH_JOINER {
                    return Some((0, Following::Ligature(ligature.joined(), selectors)));
                }
                if properties.ligature_transparent {
                    return Some((0, following));
                }
            }
            if selectors == Selectors::NONE {
                ligature_step(character, properties, ligature)
            } else {
                None
            }
        }
        _ => emoji_step(character, properties, following),
    }
}

fn ligature_step(
    character: char,
    properties: &Properties,
    ligature: Ligature,
) -> Option<(isize, Following)> {
    let tifinagh_consonant =
        TIFINAGH_CONSONANTS.contains(&character) || character == TIFINAGH_LETTER_YAWA;
    match ligature {
        Ligature::JoiningGroupAlef
            if character == ARABIC_LETTER_LAM
                || ARABIC_LETTERS_LAM_WITH_MARKS.contains(&character)
                || character == ARABIC_LETTER_LAM_WITH_BAR
                || character == ARABIC_LETTER_LAM_WITH_SMALL_ARABIC_LETTER_TAH_ABOVE
                || character == ARABIC_LETTER_LAM_WITH_SMALL_ARABIC_LETTER_JEEM_ABOVE =>
        {
            Some((0, Following::Plain))
        }
        Ligature::JoiningGroupAlef if properties.joining_transparent => {
            Some((0, Following::ligature(Ligature::JoiningGroupAlef)))
        }
        Ligature::JoinedHebrewLetterLamed if character == HEBREW_LETTER_ALEF => {
            Some((0, Following::Plain))
        }
        Ligature::KhmerCoengEligibleLetter if character == KHMER_SIGN_COENG => {
            Some((-1, Following::Plain))
        }
        Ligature::JoinedBugineseLetterYa if character == BUGINESE_VOWEL_SIGN_I => Some((
            0,
            Following::ligature(Ligature::BugineseVowelSignIJoinedLetterYa),
        )),
        Ligature::BugineseVowelSignIJoinedLetterYa if character == BUGINESE_LETTER_A => {
            Some((0, Following::Plain))
        }
        Ligature::TifinaghConsonant | Ligature::JoinedTifinaghConsonant
            if character == TIFINAGH_CONSONANT_JOINER =>
        {
            Some((1, Following::ligature(Ligature::TifinaghJoinerConsonant)))
        }
        Ligature::JoinedTifinaghConsonant if tifinagh_consonant => Some((0, Following::Plain)),
        Ligature::TifinaghJoinerConsonant if tifinagh_consonant => Some((-1, Following::Plain)),
        Ligature::LisuToneLetterMyaNaJeu if LISU_LETTER_TONES.contains(&character) => {
            Some((0, Following::Plain))
        }
        Ligature::JoinedOldTurkicLetterOrkhonI if character == OLD_TURKIC_LETTER_ORKHON_EC => {
            Some((0, Following::Plain))
        }
        _ => None,
    }
}

#[inline]
fn emoji_step(
    character: char,
    properties: &Properties,
    following: Following,
) -> Option<(isize, Following)> {
    let regional_indicator = REGIONAL_INDICATORS.contains(&character);
    match following {
        Following::EmojiModifier if properties.emoji_modifier_base => {
            Some((0, Following::EmojiPresentation))
        }
        Following::RegionalIndicator | Following::RegionalIndicators if regional_indicator => {
            Some((1, Following::RegionalIndicators))
        }
        Following::EmojiPresentation
        | Following::RegionalIndicators
        | Following::EvenJoinedRegionalIndicators
        | Following::OddJoinedRegionalIndicators
        | Following::EmojiModifier
            if character == ZERO_WIDTH_JOINER =>
        {
            Some((0, Following::JoinedEmoji))
        }
        Following::JoinedEmoji => joined_emoji_step(character, properties, regional_indicator),
        Following::JoinedRegionalIndicator | Following::OddJoinedRegionalIndicators
            if regional_indicator =>
        {
            Some((-1, Following::EvenJoinedRegionalIndicators))
        }
        Following::EvenJoinedRegionalIndicators if regional_indicator => {
            Some((3, Following::OddJoinedRegionalIndicators))
        }
        Following::Tag(run) => tag_step(character, run),
        Following::KiratRaiVowelSignE => match character {
            KIRAT_RAI_SIGN_VIRAMA | KIRAT_RAI_VOWEL_SIGN_O => Some((0, Following::Plain)),
            KIRAT_RAI_VOWEL_SIGN_E => Some((0, Following::KiratRaiVowelSignAi)),
            KIRAT_RAI_VOWEL_SIGN_AI => Some((1, Following::KiratRaiVowelSignE)),
            _ => None,
        },
        Following::KiratRaiVowelSignAi if character == KIRAT_RAI_SIGN_VIRAMA => {
            Some((0, Following::Plain))
        }
        _ => None,
    }
}

#[inline]
fn joined_emoji_step(
    character: char,
    properties: &Properties,
    regional_indicator: bool,
) -> Option<(isize, Following)> {
    if character == COMBINING_ENCLOSING_KEYCAP {
        Some((0, Following::JoinedKeycap))
    } else if regional_indicator {
        Some((1, Following::JoinedRegionalIndicator))
    } else if EMOJI_MODIFIERS.contains(&character) {
        Some((0, Following::EmojiModifier))
    } else if character == CANCEL_TAG {
        Some((0, Following::Tag(TagRun::End)))
    } else if properties.width == WidthKind::EmojiPresentation {
        Some((0, Following::EmojiPresentation))
    } else {
        None
    }
}

fn tag_step(character: char, run: TagRun) -> Option<(isize, Following)> {
    let next = if TAG_LETTERS.contains(&character) {
        run.letter().map(Following::Tag)
    } else if TAG_DIGITS.contains(&character) {
        run.digit().map(Following::Tag)
    } else if character == WAVING_BLACK_FLAG && run.completes_flag() {
        Some(Following::EmojiPresentation)
    } else {
        None
    };
    next.map(|following| (0, following))
}

pub const fn graphemes(text: &str) -> Graphemes<'_> {
    Graphemes {
        text,
        front: 0,
        back: text.len(),
    }
}

pub const fn grapheme_indices(text: &str) -> GraphemeIndices<'_> {
    GraphemeIndices {
        graphemes: graphemes(text),
    }
}

#[derive(Clone, Debug)]
pub struct Graphemes<'a> {
    text: &'a str,
    front: usize,
    back: usize,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.front == self.back {
            return None;
        }
        let start = self.front;
        let end = grapheme_end(self.text, start);
        assert!(
            end <= self.back,
            "a grapheme ends at or before the back boundary"
        );
        self.front = end;
        Some(&self.text[start..end])
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.back - self.front;
        (remaining.min(1), Some(remaining))
    }
}

impl<'a> DoubleEndedIterator for Graphemes<'a> {
    fn next_back(&mut self) -> Option<&'a str> {
        if self.front == self.back {
            return None;
        }
        let end = self.back;
        let start = grapheme_start(self.text, end);
        assert!(
            start >= self.front,
            "a grapheme starts at or after the front boundary"
        );
        self.back = start;
        Some(&self.text[start..end])
    }
}

#[derive(Clone, Debug)]
pub struct GraphemeIndices<'a> {
    graphemes: Graphemes<'a>,
}

impl<'a> Iterator for GraphemeIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<(usize, &'a str)> {
        let start = self.graphemes.front;
        self.graphemes.next().map(|grapheme| (start, grapheme))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.graphemes.size_hint()
    }
}

impl<'a> DoubleEndedIterator for GraphemeIndices<'a> {
    fn next_back(&mut self) -> Option<(usize, &'a str)> {
        let grapheme = self.graphemes.next_back()?;
        Some((self.graphemes.back, grapheme))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pair {
    Break,
    Join,
    Conjunct,
    Pictograph,
    RegionalIndicator,
}

const fn pair(previous: &Properties, next: &Properties) -> Pair {
    use GraphemeCategory::{
        CarriageReturn, ConjunctConsonant, Control, Extend, ExtendedPictographic, LeadingJamo,
        LeadingVowelSyllable, LeadingVowelTrailingSyllable, LineFeed, Prepend, RegionalIndicator,
        SpacingMark, TrailingJamo, VowelJamo, ZeroWidthJoiner,
    };
    match (previous.grapheme, next.grapheme) {
        (CarriageReturn, LineFeed) => Pair::Join,
        (Control | CarriageReturn | LineFeed, _) | (_, Control | CarriageReturn | LineFeed) => {
            Pair::Break
        }
        (
            LeadingJamo,
            LeadingJamo | VowelJamo | LeadingVowelSyllable | LeadingVowelTrailingSyllable,
        )
        | (LeadingVowelSyllable | VowelJamo, VowelJamo | TrailingJamo)
        | (LeadingVowelTrailingSyllable | TrailingJamo, TrailingJamo)
        | (_, Extend | ZeroWidthJoiner | SpacingMark)
        | (Prepend, _) => Pair::Join,
        (_, ConjunctConsonant) => Pair::Conjunct,
        (ZeroWidthJoiner, ExtendedPictographic) => Pair::Pictograph,
        (RegionalIndicator, RegionalIndicator) => Pair::RegionalIndicator,
        _ => Pair::Break,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pictograph {
    Outside,
    Extending,
    Joined,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConjunctRun {
    Outside,
    Consonant,
    Linked,
}

#[derive(Clone, Copy)]
struct Context {
    regional_indicators_odd: bool,
    pictograph: Pictograph,
    conjunct: ConjunctRun,
}

impl Context {
    const START: Self = Self {
        regional_indicators_odd: false,
        pictograph: Pictograph::Outside,
        conjunct: ConjunctRun::Outside,
    };

    fn advance(self, properties: &Properties) -> Self {
        let regional_indicators_odd = properties.grapheme == GraphemeCategory::RegionalIndicator
            && !self.regional_indicators_odd;
        let pictograph = match (properties.grapheme, self.pictograph) {
            (GraphemeCategory::ExtendedPictographic, _)
            | (GraphemeCategory::Extend, Pictograph::Extending) => Pictograph::Extending,
            (GraphemeCategory::ZeroWidthJoiner, Pictograph::Extending) => Pictograph::Joined,
            _ => Pictograph::Outside,
        };
        let conjunct = match properties.conjunct {
            Conjunct::Linker if self.conjunct == ConjunctRun::Outside => ConjunctRun::Outside,
            Conjunct::Linker => ConjunctRun::Linked,
            Conjunct::Extend => self.conjunct,
            Conjunct::None if properties.grapheme == GraphemeCategory::ConjunctConsonant => {
                ConjunctRun::Consonant
            }
            Conjunct::None => ConjunctRun::Outside,
        };
        Self {
            regional_indicators_odd,
            pictograph,
            conjunct,
        }
    }

    fn breaks(self, pair: Pair) -> bool {
        match pair {
            Pair::Break => true,
            Pair::Join => false,
            Pair::Conjunct => self.conjunct != ConjunctRun::Linked,
            Pair::Pictograph => self.pictograph != Pictograph::Joined,
            Pair::RegionalIndicator => !self.regional_indicators_odd,
        }
    }
}

fn grapheme_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let first = bytes[start];
    if first.is_ascii() {
        match bytes.get(start + 1) {
            None => return start + 1,
            Some(&next) if next.is_ascii() => {
                return start + 1 + usize::from(first == b'\r' && next == b'\n');
            }
            Some(_) => {}
        }
    }
    let mut characters = text[start..].char_indices();
    let (_, first) = characters.next().expect("a grapheme starts at a character");
    let mut previous = properties(first);
    let mut context = Context::START.advance(previous);
    for (offset, character) in characters {
        let next = properties(character);
        if context.breaks(pair(previous, next)) {
            return start + offset;
        }
        context = context.advance(next);
        previous = next;
    }
    text.len()
}

fn grapheme_start(text: &str, end: usize) -> usize {
    let bytes = text.as_bytes();
    assert!(end > 0, "a grapheme ends after the start of the text");
    if bytes[end - 1].is_ascii() {
        match end.checked_sub(2).map(|index| bytes[index]) {
            None => return 0,
            Some(before) if before.is_ascii() => {
                return end - 1 - usize::from(before == b'\r' && bytes[end - 1] == b'\n');
            }
            Some(_) => {}
        }
    }
    for (offset, _) in text[..end].char_indices().rev() {
        if offset == 0 || is_boundary(text, offset) {
            return offset;
        }
    }
    unreachable!("the first character starts at offset zero")
}

fn is_boundary(text: &str, offset: usize) -> bool {
    let before = &text[..offset];
    let previous = properties(
        before
            .chars()
            .next_back()
            .expect("a boundary follows a character"),
    );
    let next = properties(
        text[offset..]
            .chars()
            .next()
            .expect("a boundary precedes a character"),
    );
    match pair(previous, next) {
        Pair::Break => true,
        Pair::Join => false,
        Pair::Conjunct => !conjunct_linked(before),
        Pair::Pictograph => !pictograph_joined(before),
        Pair::RegionalIndicator => regional_indicators_before(before).is_multiple_of(2),
    }
}

fn conjunct_linked(before: &str) -> bool {
    let mut linkers = 0_usize;
    for character in before.chars().rev() {
        let properties = properties(character);
        match properties.conjunct {
            Conjunct::Linker => linkers += 1,
            Conjunct::Extend => {}
            Conjunct::None => {
                return linkers > 0 && properties.grapheme == GraphemeCategory::ConjunctConsonant;
            }
        }
    }
    false
}

fn pictograph_joined(before: &str) -> bool {
    let mut characters = before.chars().rev();
    let joiner = characters.next().expect("a joiner precedes the pictograph");
    assert!(
        properties(joiner).grapheme == GraphemeCategory::ZeroWidthJoiner,
        "a joiner precedes the pictograph"
    );
    for character in characters {
        match properties(character).grapheme {
            GraphemeCategory::Extend => {}
            GraphemeCategory::ExtendedPictographic => return true,
            _ => return false,
        }
    }
    false
}

fn regional_indicators_before(before: &str) -> usize {
    before
        .chars()
        .rev()
        .take_while(|&character| {
            properties(character).grapheme == GraphemeCategory::RegionalIndicator
        })
        .count()
}
