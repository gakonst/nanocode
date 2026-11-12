use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::class::{ClassBytes, ClassUnicode, Range, SCALAR_MAX};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Look {
    Start,
    End,
    StartLf,
    EndLf,
    StartCrlf,
    EndCrlf,
    WordAscii,
    WordAsciiNegate,
    WordUnicode,
    WordUnicodeNegate,
    WordStartAscii,
    WordEndAscii,
    WordStartUnicode,
    WordEndUnicode,
    WordStartHalfAscii,
    WordEndHalfAscii,
    WordStartHalfUnicode,
    WordEndHalfUnicode,
}

impl Look {
    pub const fn bit(self) -> u32 {
        1 << (self as u32)
    }

    pub const fn reversed(self) -> Look {
        match self {
            Look::Start => Look::End,
            Look::End => Look::Start,
            Look::StartLf => Look::EndLf,
            Look::EndLf => Look::StartLf,
            Look::StartCrlf => Look::EndCrlf,
            Look::EndCrlf => Look::StartCrlf,
            Look::WordStartAscii => Look::WordEndAscii,
            Look::WordEndAscii => Look::WordStartAscii,
            Look::WordStartUnicode => Look::WordEndUnicode,
            Look::WordEndUnicode => Look::WordStartUnicode,
            Look::WordStartHalfAscii => Look::WordEndHalfAscii,
            Look::WordEndHalfAscii => Look::WordStartHalfAscii,
            Look::WordStartHalfUnicode => Look::WordEndHalfUnicode,
            Look::WordEndHalfUnicode => Look::WordStartHalfUnicode,
            other => other,
        }
    }

    pub const ALL: [Look; 18] = [
        Look::Start,
        Look::End,
        Look::StartLf,
        Look::EndLf,
        Look::StartCrlf,
        Look::EndCrlf,
        Look::WordAscii,
        Look::WordAsciiNegate,
        Look::WordUnicode,
        Look::WordUnicodeNegate,
        Look::WordStartAscii,
        Look::WordEndAscii,
        Look::WordStartUnicode,
        Look::WordEndUnicode,
        Look::WordStartHalfAscii,
        Look::WordEndHalfAscii,
        Look::WordStartHalfUnicode,
        Look::WordEndHalfUnicode,
    ];
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct LookSet {
    pub bits: u32,
}

impl LookSet {
    pub const fn empty() -> LookSet {
        LookSet { bits: 0 }
    }

    pub const fn full() -> LookSet {
        LookSet { bits: !0 }
    }

    pub const fn singleton(look: Look) -> LookSet {
        LookSet { bits: look.bit() }
    }

    pub const fn contains(self, look: Look) -> bool {
        self.bits & look.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    pub const fn insert(self, look: Look) -> LookSet {
        LookSet {
            bits: self.bits | look.bit(),
        }
    }

    pub const fn union(self, other: LookSet) -> LookSet {
        LookSet {
            bits: self.bits | other.bits,
        }
    }

    pub const fn intersect(self, other: LookSet) -> LookSet {
        LookSet {
            bits: self.bits & other.bits,
        }
    }

    pub const fn subtract(self, other: LookSet) -> LookSet {
        LookSet {
            bits: self.bits & !other.bits,
        }
    }

    pub fn contains_anchor_haystack(self) -> bool {
        self.contains(Look::Start) || self.contains(Look::End)
    }

    pub fn contains_word_unicode(self) -> bool {
        self.contains(Look::WordUnicode)
            || self.contains(Look::WordUnicodeNegate)
            || self.contains(Look::WordStartUnicode)
            || self.contains(Look::WordEndUnicode)
            || self.contains(Look::WordStartHalfUnicode)
            || self.contains(Look::WordEndHalfUnicode)
    }

    pub fn contains_word(self) -> bool {
        self.contains_word_unicode()
            || self.contains(Look::WordAscii)
            || self.contains(Look::WordAsciiNegate)
            || self.contains(Look::WordStartAscii)
            || self.contains(Look::WordEndAscii)
            || self.contains(Look::WordStartHalfAscii)
            || self.contains(Look::WordEndHalfAscii)
    }

    pub fn iter(self) -> impl Iterator<Item = Look> {
        Look::ALL
            .into_iter()
            .filter(move |look| self.contains(*look))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Class {
    Unicode(ClassUnicode),
    Bytes(ClassBytes),
}

impl Class {
    fn is_empty(&self) -> bool {
        match self {
            Class::Unicode(x) => x.ranges().is_empty(),
            Class::Bytes(x) => x.ranges().is_empty(),
        }
    }

    fn literal(&self) -> Option<Vec<u8>> {
        match self {
            Class::Unicode(x) => x.literal(),
            Class::Bytes(x) => x.literal(),
        }
    }

    fn minimum_len(&self) -> Option<usize> {
        match self {
            Class::Unicode(x) => x.minimum_len(),
            Class::Bytes(x) => (!x.ranges().is_empty()).then_some(1),
        }
    }

    fn maximum_len(&self) -> Option<usize> {
        match self {
            Class::Unicode(x) => x.maximum_len(),
            Class::Bytes(x) => (!x.ranges().is_empty()).then_some(1),
        }
    }

    fn is_utf8(&self) -> bool {
        match self {
            Class::Unicode(_) => true,
            Class::Bytes(x) => x.is_ascii(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repetition {
    pub min: u32,
    pub max: Option<u32>,
    pub greedy: bool,
    pub sub: Box<Hir>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capture {
    pub index: u32,
    pub name: Option<String>,
    pub sub: Box<Hir>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HirKind {
    Empty,
    Literal(Box<[u8]>),
    Class(Class),
    Look(Look),
    Repetition(Repetition),
    Capture(Capture),
    Concat(Vec<Hir>),
    Alternation(Vec<Hir>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Properties {
    pub minimum_len: Option<usize>,
    pub maximum_len: Option<usize>,
    pub look_set: LookSet,
    pub look_set_prefix: LookSet,
    pub look_set_suffix: LookSet,
    pub utf8: bool,
    pub explicit_captures_len: usize,
    pub static_explicit_captures_len: Option<usize>,
    pub literal: bool,
    pub alternation_literal: bool,
}

#[derive(Clone, Debug)]
pub struct Hir {
    kind: HirKind,
    props: Properties,
}

impl PartialEq for Hir {
    fn eq(&self, other: &Hir) -> bool {
        self.kind == other.kind
    }
}

impl Eq for Hir {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dot {
    AnyChar,
    AnyByte,
    AnyCharExcept(char),
    AnyCharExceptCrlf,
    AnyByteExcept(u8),
    AnyByteExceptCrlf,
}

impl Hir {
    pub fn kind(&self) -> &HirKind {
        &self.kind
    }

    pub fn into_kind(self) -> HirKind {
        self.kind
    }

    pub fn properties(&self) -> &Properties {
        &self.props
    }

    pub fn empty() -> Hir {
        Hir {
            kind: HirKind::Empty,
            props: Properties::empty(),
        }
    }

    pub fn fail() -> Hir {
        let class = Class::Bytes(ClassBytes::empty());
        let props = Properties::class(&class);
        Hir {
            kind: HirKind::Class(class),
            props,
        }
    }

    pub fn literal(bytes: Vec<u8>) -> Hir {
        if bytes.is_empty() {
            return Hir::empty();
        }
        let props = Properties::literal(&bytes);
        Hir {
            kind: HirKind::Literal(bytes.into_boxed_slice()),
            props,
        }
    }

    pub fn class(class: Class) -> Hir {
        if class.is_empty() {
            return Hir::fail();
        }
        if let Some(bytes) = class.literal() {
            return Hir::literal(bytes);
        }
        let props = Properties::class(&class);
        Hir {
            kind: HirKind::Class(class),
            props,
        }
    }

    pub fn look(look: Look) -> Hir {
        Hir {
            kind: HirKind::Look(look),
            props: Properties::look(look),
        }
    }

    pub fn repetition(mut repetition: Repetition) -> Hir {
        if repetition.sub.props.maximum_len == Some(0) {
            repetition.min = repetition.min.min(1);
            repetition.max = repetition.max.map(|n| n.min(1)).or(Some(1));
        }
        if repetition.min == 0 && repetition.max == Some(0) {
            return Hir::empty();
        }
        if repetition.min == 1 && repetition.max == Some(1) {
            return *repetition.sub;
        }
        let props = Properties::repetition(&repetition);
        Hir {
            kind: HirKind::Repetition(repetition),
            props,
        }
    }

    pub fn capture(capture: Capture) -> Hir {
        let props = Properties::capture(&capture);
        Hir {
            kind: HirKind::Capture(capture),
            props,
        }
    }

    pub fn concat(subs: Vec<Hir>) -> Hir {
        let mut out: Vec<Hir> = Vec::new();
        let mut prior: Option<Vec<u8>> = None;
        let take = |sub: Hir, out: &mut Vec<Hir>, prior: &mut Option<Vec<u8>>| match sub.kind {
            HirKind::Literal(bytes) => match prior {
                Some(p) => p.extend_from_slice(&bytes),
                None => *prior = Some(bytes.into_vec()),
            },
            HirKind::Empty => {}
            kind => {
                if let Some(p) = prior.take() {
                    out.push(Hir::literal(p));
                }
                out.push(Hir {
                    kind,
                    props: sub.props,
                });
            }
        };
        for sub in subs {
            match sub.kind {
                HirKind::Concat(inner) => {
                    for sub in inner {
                        match sub.kind {
                            HirKind::Literal(_) => take(sub, &mut out, &mut prior),
                            kind => {
                                if let Some(p) = prior.take() {
                                    out.push(Hir::literal(p));
                                }
                                out.push(Hir {
                                    kind,
                                    props: sub.props,
                                });
                            }
                        }
                    }
                }
                _ => take(sub, &mut out, &mut prior),
            }
        }
        if let Some(p) = prior.take() {
            out.push(Hir::literal(p));
        }
        match out.len() {
            0 => Hir::empty(),
            1 => out.pop().expect("one element"),
            _ => {
                let props = Properties::concat(&out);
                Hir {
                    kind: HirKind::Concat(out),
                    props,
                }
            }
        }
    }

    pub fn alternation(subs: Vec<Hir>) -> Hir {
        let mut out = Vec::with_capacity(subs.len());
        for sub in subs {
            match sub.kind {
                HirKind::Alternation(inner) => out.extend(inner),
                kind => out.push(Hir {
                    kind,
                    props: sub.props,
                }),
            }
        }
        if out.is_empty() {
            return Hir::fail();
        }
        if out.len() == 1 {
            return out.pop().expect("one element");
        }
        if let Some(chars) = singleton_chars(&out) {
            return Hir::class(Class::Unicode(ClassUnicode::new(
                chars.into_iter().map(|c| Range { start: c, end: c }),
            )));
        }
        if let Some(bytes) = singleton_bytes(&out) {
            return Hir::class(Class::Bytes(ClassBytes::new(
                bytes.into_iter().map(|b| Range { start: b, end: b }),
            )));
        }
        if let Some(class) = class_chars(&out) {
            return Hir::class(class);
        }
        if let Some(class) = class_bytes(&out) {
            return Hir::class(class);
        }
        let out = match lift_common_prefix(out) {
            Ok(hir) => return hir,
            Err(unchanged) => unchanged,
        };
        let props = Properties::alternation(&out);
        Hir {
            kind: HirKind::Alternation(out),
            props,
        }
    }

    pub fn dot(dot: Dot) -> Hir {
        match dot {
            Dot::AnyChar => Hir::class(Class::Unicode(ClassUnicode::new([Range {
                start: 0,
                end: SCALAR_MAX,
            }]))),
            Dot::AnyByte => Hir::class(Class::Bytes(ClassBytes::new([Range {
                start: 0,
                end: 0xFF,
            }]))),
            Dot::AnyCharExcept(c) => {
                let mut class = ClassUnicode::new([Range {
                    start: u32::from(c),
                    end: u32::from(c),
                }]);
                class.negate();
                Hir::class(Class::Unicode(class))
            }
            Dot::AnyCharExceptCrlf => Hir::class(Class::Unicode(ClassUnicode::new([
                Range {
                    start: 0,
                    end: 0x09,
                },
                Range {
                    start: 0x0B,
                    end: 0x0C,
                },
                Range {
                    start: 0x0E,
                    end: SCALAR_MAX,
                },
            ]))),
            Dot::AnyByteExcept(b) => {
                let mut class = ClassBytes::new([Range {
                    start: u32::from(b),
                    end: u32::from(b),
                }]);
                class.negate();
                Hir::class(Class::Bytes(class))
            }
            Dot::AnyByteExceptCrlf => Hir::class(Class::Bytes(ClassBytes::new([
                Range {
                    start: 0,
                    end: 0x09,
                },
                Range {
                    start: 0x0B,
                    end: 0x0C,
                },
                Range {
                    start: 0x0E,
                    end: 0xFF,
                },
            ]))),
        }
    }
}

fn singleton_chars(hirs: &[Hir]) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for hir in hirs {
        let HirKind::Literal(bytes) = &hir.kind else {
            return None;
        };
        let s = core::str::from_utf8(bytes).ok()?;
        let mut chars = s.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        out.push(u32::from(c));
    }
    Some(out)
}

fn singleton_bytes(hirs: &[Hir]) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for hir in hirs {
        let HirKind::Literal(bytes) = &hir.kind else {
            return None;
        };
        if bytes.len() != 1 {
            return None;
        }
        out.push(u32::from(bytes[0]));
    }
    Some(out)
}

fn class_chars(hirs: &[Hir]) -> Option<Class> {
    let mut class = ClassUnicode::empty();
    for hir in hirs {
        match &hir.kind {
            HirKind::Class(Class::Unicode(x)) => class.union(x),
            HirKind::Class(Class::Bytes(x)) => class.union(&x.to_unicode_class()?),
            _ => return None,
        }
    }
    Some(Class::Unicode(class))
}

fn class_bytes(hirs: &[Hir]) -> Option<Class> {
    let mut class = ClassBytes::empty();
    for hir in hirs {
        match &hir.kind {
            HirKind::Class(Class::Unicode(x)) => class.union(&x.to_byte_class()?),
            HirKind::Class(Class::Bytes(x)) => class.union(x),
            _ => return None,
        }
    }
    Some(Class::Bytes(class))
}

fn lift_common_prefix(hirs: Vec<Hir>) -> Result<Hir, Vec<Hir>> {
    if hirs.len() <= 1 {
        return Err(hirs);
    }
    let HirKind::Concat(first) = &hirs[0].kind else {
        return Err(hirs);
    };
    if first.is_empty() {
        return Err(hirs);
    }
    let mut common = first.len();
    for hir in &hirs[1..] {
        let HirKind::Concat(xs) = &hir.kind else {
            return Err(hirs);
        };
        common = first[..common]
            .iter()
            .zip(xs.iter())
            .take_while(|(x, y)| x == y)
            .count();
        if common == 0 {
            return Err(hirs);
        }
    }
    let mut prefix = Vec::new();
    let mut suffixes = Vec::new();
    for hir in hirs {
        let HirKind::Concat(mut xs) = hir.kind else {
            unreachable!("every alternative is a concatenation")
        };
        let rest = xs.split_off(common);
        suffixes.push(Hir::concat(rest));
        if prefix.is_empty() {
            prefix = xs;
        }
    }
    prefix.push(Hir::alternation(suffixes));
    Ok(Hir::concat(prefix))
}

impl Properties {
    fn empty() -> Properties {
        Properties {
            minimum_len: Some(0),
            maximum_len: Some(0),
            look_set: LookSet::empty(),
            look_set_prefix: LookSet::empty(),
            look_set_suffix: LookSet::empty(),
            utf8: true,
            explicit_captures_len: 0,
            static_explicit_captures_len: Some(0),
            literal: false,
            alternation_literal: false,
        }
    }

    fn literal(bytes: &[u8]) -> Properties {
        Properties {
            minimum_len: Some(bytes.len()),
            maximum_len: Some(bytes.len()),
            utf8: core::str::from_utf8(bytes).is_ok(),
            literal: true,
            alternation_literal: true,
            ..Properties::empty()
        }
    }

    fn class(class: &Class) -> Properties {
        Properties {
            minimum_len: class.minimum_len(),
            maximum_len: class.maximum_len(),
            utf8: class.is_utf8(),
            ..Properties::empty()
        }
    }

    fn look(look: Look) -> Properties {
        let set = LookSet::singleton(look);
        Properties {
            look_set: set,
            look_set_prefix: set,
            look_set_suffix: set,
            ..Properties::empty()
        }
    }

    fn repetition(repetition: &Repetition) -> Properties {
        let p = repetition.sub.props;
        let minimum_len = p
            .minimum_len
            .map(|child| child.saturating_mul(repetition.min as usize));
        let maximum_len = repetition
            .max
            .and_then(|max| p.maximum_len?.checked_mul(max as usize));
        let mut out = Properties {
            minimum_len,
            maximum_len,
            look_set: p.look_set,
            look_set_prefix: LookSet::empty(),
            look_set_suffix: LookSet::empty(),
            utf8: p.utf8,
            explicit_captures_len: p.explicit_captures_len,
            static_explicit_captures_len: p.static_explicit_captures_len,
            literal: false,
            alternation_literal: false,
        };
        if repetition.min > 0 {
            out.look_set_prefix = p.look_set_prefix;
            out.look_set_suffix = p.look_set_suffix;
        }
        if repetition.min == 0 && out.static_explicit_captures_len.is_some_and(|n| n > 0) {
            out.static_explicit_captures_len = if repetition.max == Some(0) {
                Some(0)
            } else {
                None
            };
        }
        out
    }

    fn capture(capture: &Capture) -> Properties {
        let p = capture.sub.props;
        Properties {
            explicit_captures_len: p.explicit_captures_len.saturating_add(1),
            static_explicit_captures_len: p
                .static_explicit_captures_len
                .map(|n| n.saturating_add(1)),
            literal: false,
            alternation_literal: false,
            ..p
        }
    }

    fn concat(subs: &[Hir]) -> Properties {
        let mut out = Properties {
            literal: true,
            alternation_literal: true,
            ..Properties::empty()
        };
        for sub in subs {
            let p = sub.props;
            out.look_set = out.look_set.union(p.look_set);
            out.utf8 = out.utf8 && p.utf8;
            out.explicit_captures_len = out
                .explicit_captures_len
                .saturating_add(p.explicit_captures_len);
            out.static_explicit_captures_len = match (
                p.static_explicit_captures_len,
                out.static_explicit_captures_len,
            ) {
                (Some(a), Some(b)) => Some(a.saturating_add(b)),
                _ => None,
            };
            out.literal = out.literal && p.literal;
            out.alternation_literal = out.alternation_literal && p.alternation_literal;
            if let Some(min) = out.minimum_len {
                out.minimum_len = p.minimum_len.map(|n| min.saturating_add(n));
            }
            if let Some(max) = out.maximum_len {
                out.maximum_len = p.maximum_len.and_then(|n| max.checked_add(n));
            }
        }
        for sub in subs {
            out.look_set_prefix = out.look_set_prefix.union(sub.props.look_set_prefix);
            if sub.props.maximum_len.is_none_or(|n| n > 0) {
                break;
            }
        }
        for sub in subs.iter().rev() {
            out.look_set_suffix = out.look_set_suffix.union(sub.props.look_set_suffix);
            if sub.props.maximum_len.is_none_or(|n| n > 0) {
                break;
            }
        }
        out
    }

    fn alternation(subs: &[Hir]) -> Properties {
        let mut out = Properties {
            minimum_len: None,
            maximum_len: None,
            look_set: LookSet::empty(),
            look_set_prefix: if subs.is_empty() {
                LookSet::empty()
            } else {
                LookSet::full()
            },
            look_set_suffix: if subs.is_empty() {
                LookSet::empty()
            } else {
                LookSet::full()
            },
            utf8: true,
            explicit_captures_len: 0,
            static_explicit_captures_len: subs
                .first()
                .and_then(|s| s.props.static_explicit_captures_len),
            literal: false,
            alternation_literal: true,
        };
        let (mut min_poisoned, mut max_poisoned) = (false, false);
        for sub in subs {
            let p = sub.props;
            out.look_set = out.look_set.union(p.look_set);
            out.look_set_prefix = out.look_set_prefix.intersect(p.look_set_prefix);
            out.look_set_suffix = out.look_set_suffix.intersect(p.look_set_suffix);
            out.utf8 = out.utf8 && p.utf8;
            out.explicit_captures_len = out
                .explicit_captures_len
                .saturating_add(p.explicit_captures_len);
            if out.static_explicit_captures_len != p.static_explicit_captures_len {
                out.static_explicit_captures_len = None;
            }
            out.alternation_literal = out.alternation_literal && p.literal;
            if !min_poisoned {
                match p.minimum_len {
                    Some(n) => {
                        if out.minimum_len.is_none_or(|m| n < m) {
                            out.minimum_len = Some(n);
                        }
                    }
                    None => {
                        out.minimum_len = None;
                        min_poisoned = true;
                    }
                }
            }
            if !max_poisoned {
                match p.maximum_len {
                    Some(n) => {
                        if out.maximum_len.is_none_or(|m| n > m) {
                            out.maximum_len = Some(n);
                        }
                    }
                    None => {
                        out.maximum_len = None;
                        max_poisoned = true;
                    }
                }
            }
        }
        out
    }
}
