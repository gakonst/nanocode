use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use memchr::memmem;
use nanocode_sys::teddy::{TEDDY_BUCKETS, TEDDY_PREFIX_BYTES_MAX, Teddy};

use crate::hir::{Class, Hir, HirKind, Look};

const LIMIT_CLASS: usize = 10;
const LIMIT_REPEAT: u32 = 10;
const LIMIT_LITERAL_LENGTH: usize = 100;
const LIMIT_TOTAL: usize = 64;
const TRIMMED_PREFIX_BYTES: usize = 4;
const TRIMMED_SUFFIX_BYTES: [usize; 4] = [4, 3, 2, 1];
const RARE_BYTES_MAX: usize = 3;
const BYTE_RANGES_MAX: usize = 3;
const WORD_BYTES: usize = 8;
const WORD_ONES: u64 = 0x0101_0101_0101_0101;
const WORD_HIGH_BITS: u64 = 0x8080_8080_8080_8080;
const ASCII_MAX: u8 = 0x7F;
const ASCII_CASE_BIT: u8 = 0x20;
const RARE_BYTE_RANK_MAX: usize = 230;
const PACKED_BLOCK_BYTES: usize = 64;
const PACKED_GROUP_TESTS: usize = 3;
const PACKED_GROUPS_MAX: usize = 8;
const PACKED_REACH_BYTES_MAX: usize = 16;
const TEDDY_PREFIX_BYTES_MIN: usize = 2;
const TEDDY_RARE_BYTE_RANK_MAX: usize = 210;

const BYTE_FREQUENCY_RANK: [u8; 256] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 246, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
    25, 26, 27, 28, 29, 30, 255, 192, 227, 194, 168, 180, 216, 213, 241, 242, 179, 174, 236, 215,
    233, 221, 214, 209, 201, 190, 187, 177, 185, 172, 183, 170, 235, 226, 200, 228, 217, 181, 164,
    208, 184, 204, 195, 212, 193, 173, 175, 203, 165, 171, 189, 191, 202, 211, 206, 162, 207, 218,
    210, 178, 188, 169, 166, 167, 163, 198, 176, 197, 160, 234, 205, 249, 230, 244, 240, 254, 239,
    231, 232, 247, 196, 220, 245, 237, 250, 248, 238, 182, 252, 251, 253, 243, 222, 225, 219, 229,
    186, 224, 199, 223, 158, 31, 159, 122, 146, 116, 107, 119, 152, 94, 138, 136, 82, 92, 95, 120,
    101, 97, 113, 83, 154, 133, 156, 125, 134, 141, 147, 144, 81, 98, 139, 121, 108, 150, 112, 89,
    115, 96, 102, 109, 157, 117, 78, 149, 84, 110, 143, 118, 93, 103, 104, 135, 131, 85, 99, 145,
    90, 151, 130, 111, 123, 126, 105, 127, 106, 142, 32, 33, 155, 153, 86, 87, 34, 35, 36, 37, 38,
    39, 40, 41, 124, 128, 91, 100, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 137,
    161, 88, 132, 140, 114, 57, 79, 58, 80, 59, 60, 61, 62, 129, 148, 63, 64, 65, 66, 67, 68, 69,
    70, 71, 72, 73, 74, 75, 76, 77,
];

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Literal {
    bytes: Vec<u8>,
    exact: bool,
}

#[derive(Clone, Debug)]
struct Seq {
    literals: Option<Vec<Literal>>,
}

impl Seq {
    fn infinite() -> Seq {
        Seq { literals: None }
    }

    fn singleton(bytes: Vec<u8>, exact: bool) -> Seq {
        Seq {
            literals: Some(vec![Literal { bytes, exact }]),
        }
    }

    fn empty_set() -> Seq {
        Seq {
            literals: Some(Vec::new()),
        }
    }

    fn make_inexact(&mut self) {
        if let Some(literals) = &mut self.literals {
            for literal in literals {
                literal.exact = false;
            }
        }
    }

    fn all_inexact(&self) -> bool {
        self.literals
            .as_ref()
            .is_some_and(|literals| literals.iter().all(|l| !l.exact))
    }

    fn keep_first_bytes(&mut self, len: usize) {
        if let Some(literals) = &mut self.literals {
            for literal in literals {
                if literal.bytes.len() > len {
                    literal.bytes.truncate(len);
                    literal.exact = false;
                }
            }
        }
    }

    fn over_limit(&self) -> bool {
        self.literals.as_ref().is_some_and(|l| l.len() > LIMIT_TOTAL)
    }

    fn dedup(&mut self) {
        if let Some(literals) = &mut self.literals {
            let mut out: Vec<Literal> = Vec::with_capacity(literals.len());
            for literal in literals.drain(..) {
                if let Some(existing) = out.iter_mut().find(|l| l.bytes == literal.bytes) {
                    existing.exact = existing.exact && literal.exact;
                    continue;
                }
                out.push(literal);
            }
            *literals = out;
        }
    }
}

fn union(mut a: Seq, b: Seq) -> Seq {
    let (Some(x), Some(y)) = (&mut a.literals, b.literals) else {
        return Seq::infinite();
    };
    x.extend(y);
    a.dedup();
    if a.over_limit() {
        a.keep_first_bytes(TRIMMED_PREFIX_BYTES);
        a.dedup();
        if a.over_limit() {
            return Seq::infinite();
        }
    }
    a
}

fn cross(a: Seq, b: Seq) -> Seq {
    let Some(left) = a.literals else {
        return Seq::infinite();
    };
    if left.iter().all(|l| !l.exact) {
        return Seq {
            literals: Some(left),
        };
    }
    let Some(right) = b.literals else {
        let mut out = Seq {
            literals: Some(left),
        };
        out.make_inexact();
        return out;
    };
    let exact_count = left.iter().filter(|l| l.exact).count();
    let fits = |right: &[Literal]| exact_count * right.len() + left.len() <= LIMIT_TOTAL;
    let right = if fits(&right) {
        right
    } else {
        let trimmed = TRIMMED_SUFFIX_BYTES.iter().find_map(|&len| {
            let mut trimmed = Seq {
                literals: Some(right.clone()),
            };
            trimmed.keep_first_bytes(len);
            trimmed.dedup();
            trimmed.literals.filter(|literals| fits(literals))
        });
        let Some(trimmed) = trimmed else {
            let mut out = Seq {
                literals: Some(left),
            };
            out.make_inexact();
            return out;
        };
        trimmed
    };
    let mut out = Vec::new();
    for literal in left {
        if !literal.exact {
            out.push(literal);
            continue;
        }
        for suffix in &right {
            let mut bytes = literal.bytes.clone();
            bytes.extend_from_slice(&suffix.bytes);
            let mut exact = suffix.exact;
            if bytes.len() > LIMIT_LITERAL_LENGTH {
                bytes.truncate(LIMIT_LITERAL_LENGTH);
                exact = false;
            }
            out.push(Literal { bytes, exact });
        }
    }
    let mut seq = Seq {
        literals: Some(out),
    };
    seq.dedup();
    seq
}

fn class_literals(class: &Class) -> Seq {
    let ranges: Vec<(u32, u32)> = match class {
        Class::Unicode(c) => c.ranges().iter().map(|r| (r.start, r.end)).collect(),
        Class::Bytes(c) => c.ranges().iter().map(|r| (r.start, r.end)).collect(),
    };
    let count: usize = ranges.iter().map(|&(s, e)| (e - s + 1) as usize).sum();
    if count > LIMIT_CLASS {
        return Seq::infinite();
    }
    let mut out = Seq::empty_set();
    let literals = out.literals.as_mut().expect("finite");
    for (start, end) in ranges {
        for value in start..=end {
            match class {
                Class::Unicode(_) => {
                    let Some(c) = char::from_u32(value) else {
                        continue;
                    };
                    let mut buffer = [0; 4];
                    literals.push(Literal {
                        bytes: c.encode_utf8(&mut buffer).as_bytes().to_vec(),
                        exact: true,
                    });
                }
                Class::Bytes(_) => literals.push(Literal {
                    bytes: vec![value as u8],
                    exact: true,
                }),
            }
        }
    }
    out
}

fn prefixes(hir: &Hir) -> Seq {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => Seq::singleton(Vec::new(), true),
        HirKind::Literal(bytes) => {
            let mut bytes = bytes.to_vec();
            let mut exact = true;
            if bytes.len() > LIMIT_LITERAL_LENGTH {
                bytes.truncate(LIMIT_LITERAL_LENGTH);
                exact = false;
            }
            Seq::singleton(bytes, exact)
        }
        HirKind::Class(class) => class_literals(class),
        HirKind::Capture(capture) => prefixes(&capture.sub),
        HirKind::Repetition(repetition) => {
            let sub = prefixes(&repetition.sub);
            if repetition.min == 0 {
                let mut sub = sub;
                if repetition.max != Some(1) {
                    sub.make_inexact();
                }
                return union(sub, Seq::singleton(Vec::new(), true));
            }
            let mut seq = Seq::singleton(Vec::new(), true);
            for _ in 0..repetition.min.min(LIMIT_REPEAT) {
                if seq.all_inexact() {
                    break;
                }
                seq = cross(seq, sub.clone());
            }
            if repetition.max != Some(repetition.min) || repetition.min > LIMIT_REPEAT {
                seq.make_inexact();
            }
            seq
        }
        HirKind::Concat(subs) => {
            let mut seq = Seq::singleton(Vec::new(), true);
            for sub in subs {
                if seq.literals.is_none() || seq.all_inexact() {
                    break;
                }
                seq = cross(seq, prefixes(sub));
            }
            seq
        }
        HirKind::Alternation(subs) => {
            let mut seq = Seq::empty_set();
            for sub in subs {
                seq = union(seq, prefixes(sub));
                if seq.literals.is_none() {
                    break;
                }
            }
            seq
        }
    }
}

fn usable(seq: &Seq) -> Option<Vec<Vec<u8>>> {
    let literals = seq.literals.as_ref()?;
    if literals.is_empty() || literals.iter().any(|l| l.bytes.is_empty()) {
        return None;
    }
    let mut out: Vec<Vec<u8>> = literals.iter().map(|l| l.bytes.clone()).collect();
    out.sort();
    out.dedup();
    let mut minimal: Vec<Vec<u8>> = Vec::new();
    for literal in out {
        if minimal.iter().any(|m| literal.starts_with(m)) {
            continue;
        }
        minimal.push(literal);
    }
    Some(minimal)
}

fn score(literals: &[Vec<u8>]) -> usize {
    let shortest = literals.iter().map(Vec::len).min().unwrap_or(0);
    let rarest = literals
        .iter()
        .map(|l| {
            l.iter()
                .map(|&b| BYTE_FREQUENCY_RANK[b as usize] as usize)
                .min()
                .unwrap_or(usize::MAX)
        })
        .max()
        .unwrap_or(usize::MAX);
    shortest.min(LIMIT_LITERAL_LENGTH) * 1024
        + (256 - rarest.min(256)) * 4
        + 4usize.saturating_sub(literals.len().min(4))
}

#[derive(Clone, Debug)]
pub enum Prefilter {
    Memmem(Box<memmem::Finder<'static>>),
    Bytes {
        needles: Vec<u8>,
        offset: usize,
    },
    Ranges {
        ranges: Vec<(u8, u8)>,
        offset: usize,
    },
    Packed(Packed),
    Teddy(Box<TeddySet>),
    NonAscii,
    LineStart(u8),
}

#[derive(Clone, Debug)]
struct CaseGroup {
    key: Vec<u8>,
    members: Vec<Vec<u8>>,
    every_case: bool,
}

impl CaseGroup {
    fn matches(&self, rest: &[u8]) -> bool {
        rest.len() >= self.key.len()
            && rest[..self.key.len()].eq_ignore_ascii_case(&self.key)
            && (self.every_case || self.members.iter().any(|member| rest.starts_with(member)))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Verified {
    Nothing,
    One(usize),
    Several,
}

#[derive(Clone, Debug)]
pub struct TeddySet {
    teddy: Teddy,
    groups: Vec<CaseGroup>,
    buckets: [Vec<usize>; TEDDY_BUCKETS],
}

impl TeddySet {
    fn new(literals: &[Vec<u8>]) -> Option<TeddySet> {
        let shortest = literals.iter().map(Vec::len).min()?;
        if literals.len() < 2 || shortest < TEDDY_PREFIX_BYTES_MIN {
            return None;
        }
        let prefix_len = shortest.min(TEDDY_PREFIX_BYTES_MAX);
        let mut groups: Vec<CaseGroup> = Vec::new();
        for literal in literals {
            let key = literal.to_ascii_lowercase();
            match groups.iter_mut().find(|group| group.key == key) {
                Some(group) => {
                    if !group.members.contains(literal) {
                        group.members.push(literal.clone());
                    }
                }
                None => groups.push(CaseGroup {
                    key,
                    members: vec![literal.clone()],
                    every_case: false,
                }),
            }
        }
        for group in &mut groups {
            let letters = group.key.iter().filter(|b| b.is_ascii_alphabetic()).count();
            group.every_case = u32::try_from(letters)
                .ok()
                .and_then(|letters| 1usize.checked_shl(letters))
                .is_some_and(|variants| variants == group.members.len());
        }
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by(|&a, &b| groups[a].key[..prefix_len].cmp(&groups[b].key[..prefix_len]));
        let mut buckets: [Vec<usize>; TEDDY_BUCKETS] = Default::default();
        let mut prefixes: Vec<(usize, &[u8])> = Vec::new();
        for (rank, &group) in order.iter().enumerate() {
            let bucket = rank * TEDDY_BUCKETS / groups.len();
            buckets[bucket].push(group);
            for member in &groups[group].members {
                prefixes.push((bucket, &member[..prefix_len]));
            }
        }
        let teddy = Teddy::new(&prefixes, prefix_len)?;
        Some(TeddySet {
            teddy,
            groups,
            buckets,
        })
    }

    fn verify(&self, haystack: &[u8], at: usize, buckets: u8) -> Verified {
        let rest = &haystack[at..];
        let mut found = Verified::Nothing;
        let mut pending = buckets;
        while pending != 0 {
            let bucket = pending.trailing_zeros() as usize;
            pending &= pending - 1;
            for &group in &self.buckets[bucket] {
                let group = &self.groups[group];
                if group.matches(rest) {
                    if found != Verified::Nothing {
                        return Verified::Several;
                    }
                    found = Verified::One(group.key.len());
                }
            }
        }
        found
    }

    fn find(&self, haystack: &[u8], start: usize) -> Option<(usize, Verified)> {
        let mut at = start;
        loop {
            let (candidate, buckets) = self.teddy.find(haystack, at)?;
            match self.verify(haystack, candidate, buckets) {
                Verified::Nothing => at = candidate + 1,
                verified => return Some((candidate, verified)),
            }
        }
    }
}

fn find_non_ascii(haystack: &[u8]) -> Option<usize> {
    let (chunks, remainder) = haystack.as_chunks::<WORD_BYTES>();
    for (index, chunk) in chunks.iter().enumerate() {
        let high = u64::from_le_bytes(*chunk) & WORD_HIGH_BITS;
        if high != 0 {
            return Some(index * WORD_BYTES + high.trailing_zeros() as usize / WORD_BYTES);
        }
    }
    remainder
        .iter()
        .position(|&byte| byte > ASCII_MAX)
        .map(|index| chunks.len() * WORD_BYTES + index)
}

fn may_start_ascii(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => false,
        HirKind::Literal(bytes) => bytes.first().is_none_or(|&byte| byte <= ASCII_MAX),
        HirKind::Class(Class::Unicode(class)) => class
            .ranges()
            .iter()
            .any(|range| range.start <= u32::from(ASCII_MAX)),
        HirKind::Class(Class::Bytes(class)) => class
            .ranges()
            .iter()
            .any(|range| range.start <= u32::from(ASCII_MAX)),
        HirKind::Repetition(repetition) => may_start_ascii(&repetition.sub),
        HirKind::Capture(capture) => may_start_ascii(&capture.sub),
        HirKind::Concat(subs) => {
            for sub in subs {
                if may_start_ascii(sub) {
                    return true;
                }
                if sub.properties().minimum_len != Some(0) {
                    return false;
                }
            }
            true
        }
        HirKind::Alternation(subs) => subs.iter().any(may_start_ascii),
    }
}

pub fn first_byte_prefilter(hir: &Hir) -> Option<Prefilter> {
    if hir
        .properties()
        .minimum_len
        .is_none_or(|length| length == 0)
        || may_start_ascii(hir)
    {
        return None;
    }
    Some(Prefilter::NonAscii)
}

pub fn line_start_prefilter(hir: &Hir, line_terminator: u8) -> Option<Prefilter> {
    hir.properties()
        .look_set_prefix
        .contains(Look::StartLf)
        .then_some(Prefilter::LineStart(line_terminator))
}

#[derive(Clone, Copy, Debug)]
struct ByteTest {
    offset: usize,
    mask: u8,
    value: u8,
}

impl ByteTest {
    fn of(members: &[&Vec<u8>], key: &[u8], offset: usize) -> ByteTest {
        let first = members[0][offset];
        if members.iter().all(|member| member[offset] == first) {
            return ByteTest {
                offset,
                mask: u8::MAX,
                value: first,
            };
        }
        assert!(
            key[offset].is_ascii_lowercase(),
            "literals sharing an ASCII-lowercased key differ only in the case of ASCII letters"
        );
        ByteTest {
            offset,
            mask: !ASCII_CASE_BIT,
            value: key[offset].to_ascii_uppercase(),
        }
    }

    fn rank(key: &[u8], offset: usize) -> usize {
        let lower = key[offset];
        let upper = lower.to_ascii_uppercase();
        usize::from(BYTE_FREQUENCY_RANK[usize::from(lower)])
            .max(usize::from(BYTE_FREQUENCY_RANK[usize::from(upper)]))
    }

    #[inline(always)]
    fn holds(self, byte: u8) -> bool {
        byte & self.mask == self.value
    }
}

type GroupTests = [ByteTest; PACKED_GROUP_TESTS];

#[derive(Clone, Debug)]
pub struct Packed {
    groups: Vec<GroupTests>,
    reach: usize,
}

impl Packed {
    fn new(literals: &[Vec<u8>]) -> Option<Packed> {
        let mut keyed: Vec<(Vec<u8>, Vec<&Vec<u8>>)> = Vec::new();
        for literal in literals {
            let key = literal.to_ascii_lowercase();
            match keyed.iter_mut().find(|(existing, _)| *existing == key) {
                Some((_, members)) => members.push(literal),
                None => keyed.push((key, vec![literal])),
            }
            if keyed.len() > PACKED_GROUPS_MAX {
                return None;
            }
        }
        let mut groups = Vec::with_capacity(keyed.len());
        let mut reach = 0;
        for (key, members) in &keyed {
            assert!(!key.is_empty(), "a packed group's literals are not empty");
            let mut offsets: Vec<usize> = (0..key.len().min(PACKED_REACH_BYTES_MAX)).collect();
            offsets.sort_by_key(|&offset| (ByteTest::rank(key, offset), offset));
            offsets.truncate(PACKED_GROUP_TESTS);
            offsets.sort_unstable();
            let tests: GroupTests = core::array::from_fn(|index| {
                let offset = offsets[index.min(offsets.len() - 1)];
                ByteTest::of(members, key, offset)
            });
            reach = reach.max(offsets[offsets.len() - 1]);
            groups.push(tests);
        }
        Some(Packed { groups, reach })
    }

    #[inline(always)]
    fn group_hits(window: &[u8], group: &GroupTests) -> [bool; PACKED_BLOCK_BYTES] {
        let lanes = |test: ByteTest| -> &[u8; PACKED_BLOCK_BYTES] {
            window[test.offset..test.offset + PACKED_BLOCK_BYTES]
                .try_into()
                .expect("a packed window holds a block past every test offset")
        };
        let [first, second, third] = *group;
        let mut hits = [false; PACKED_BLOCK_BYTES];
        for (((hit, &a), &b), &c) in hits
            .iter_mut()
            .zip(lanes(first))
            .zip(lanes(second))
            .zip(lanes(third))
        {
            *hit = first.holds(a) & second.holds(b) & third.holds(c);
        }
        hits
    }

    fn starts_at(&self, haystack: &[u8], start: usize) -> bool {
        self.groups.iter().any(|group| {
            group.iter().all(|test| {
                haystack
                    .get(start + test.offset)
                    .is_some_and(|&byte| test.holds(byte))
            })
        })
    }

    fn find(&self, haystack: &[u8], start: usize) -> Option<usize> {
        let mut at = start;
        while at + PACKED_BLOCK_BYTES + self.reach <= haystack.len() {
            let window = &haystack[at..at + PACKED_BLOCK_BYTES + self.reach];
            let mut any = [0u8; PACKED_BLOCK_BYTES];
            for group in &self.groups {
                let hits = Packed::group_hits(window, group);
                for (slot, hit) in any.iter_mut().zip(hits) {
                    *slot |= u8::from(hit);
                }
            }
            if any.iter().fold(0, |folded, &hit| folded | hit) != 0 {
                let lane = any
                    .iter()
                    .position(|&hit| hit != 0)
                    .expect("a block with a hit has a first hit");
                return Some(at + lane);
            }
            at += PACKED_BLOCK_BYTES;
        }
        (at..haystack.len()).find(|&candidate| self.starts_at(haystack, candidate))
    }
}

type NarrowestRanges = (usize, Vec<(u8, u8)>, usize);

fn ascii_ranges(bytes: &[u8]) -> Option<Vec<(u8, u8)>> {
    let mut ranges: Vec<(u8, u8)> = Vec::new();
    for &b in bytes {
        if b == 0 || b > ASCII_MAX {
            return None;
        }
        match ranges.last_mut() {
            Some(last) if last.1 + 1 == b => last.1 = b,
            _ => ranges.push((b, b)),
        }
        if ranges.len() > BYTE_RANGES_MAX {
            return None;
        }
    }
    Some(ranges)
}

fn find_in_ranges(haystack: &[u8], ranges: &[(u8, u8)]) -> Option<usize> {
    let (chunks, remainder) = haystack.as_chunks::<WORD_BYTES>();
    let mut base = 0;
    for chunk in chunks {
        let word = u64::from_le_bytes(*chunk);
        let low = word & (WORD_ONES * u64::from(ASCII_MAX));
        let mut mask = 0;
        for &(first, last) in ranges {
            let above = low + WORD_ONES * (u64::from(ASCII_MAX) - (u64::from(first) - 1));
            let below =
                (WORD_ONES * (u64::from(ASCII_MAX) + u64::from(last) + 1)).wrapping_sub(low);
            mask |= below & !word & above & WORD_HIGH_BITS;
        }
        if mask != 0 {
            return Some(base + mask.trailing_zeros() as usize / WORD_BYTES);
        }
        base += WORD_BYTES;
    }
    remainder
        .iter()
        .position(|b| {
            ranges
                .iter()
                .any(|&(first, last)| (first..=last).contains(b))
        })
        .map(|i| base + i)
}

impl Prefilter {
    fn from_literals(literals: &[Vec<u8>]) -> Option<Prefilter> {
        Prefilter::from_literals_with(literals, true)
    }

    fn from_literals_with(literals: &[Vec<u8>], teddy: bool) -> Option<Prefilter> {
        if literals.len() == 1 {
            let literal = &literals[0];
            if literal.len() == 1 {
                return Some(Prefilter::Bytes {
                    needles: vec![literal[0]],
                    offset: 0,
                });
            }
            return Some(Prefilter::Memmem(Box::new(
                memmem::Finder::new(literal).into_owned(),
            )));
        }
        let shortest = literals.iter().map(Vec::len).min()?;
        let mut best: Option<(usize, Vec<u8>, usize)> = None;
        for offset in 0..shortest {
            let mut needles: Vec<u8> = literals.iter().map(|l| l[offset]).collect();
            needles.sort_unstable();
            needles.dedup();
            if needles.len() > RARE_BYTES_MAX {
                continue;
            }
            let worst = needles
                .iter()
                .map(|&b| BYTE_FREQUENCY_RANK[b as usize] as usize)
                .max()
                .unwrap_or(0);
            let cost = worst * RARE_BYTES_MAX + needles.len();
            if best.as_ref().is_none_or(|(c, _, _)| cost < *c) {
                best = Some((cost, needles, offset));
            }
        }
        if let Some((cost, needles, offset)) = best {
            let worst = cost / RARE_BYTES_MAX;
            if worst <= TEDDY_RARE_BYTE_RANK_MAX {
                return Some(Prefilter::Bytes { needles, offset });
            }
            if teddy && let Some(teddy) = TeddySet::new(literals) {
                return Some(Prefilter::Teddy(Box::new(teddy)));
            }
            if worst <= RARE_BYTE_RANK_MAX {
                return Some(Prefilter::Bytes { needles, offset });
            }
            if let Some(packed) = Packed::new(literals) {
                return Some(Prefilter::Packed(packed));
            }
            return Some(Prefilter::Bytes { needles, offset });
        }
        if teddy && let Some(teddy) = TeddySet::new(literals) {
            return Some(Prefilter::Teddy(Box::new(teddy)));
        }
        if let Some(packed) = Packed::new(literals) {
            return Some(Prefilter::Packed(packed));
        }
        let mut narrowest: Option<NarrowestRanges> = None;
        for offset in 0..shortest {
            let mut bytes: Vec<u8> = literals.iter().map(|l| l[offset]).collect();
            bytes.sort_unstable();
            bytes.dedup();
            let Some(ranges) = ascii_ranges(&bytes) else {
                continue;
            };
            if narrowest
                .as_ref()
                .is_none_or(|(count, _, _)| bytes.len() < *count)
            {
                narrowest = Some((bytes.len(), ranges, offset));
            }
        }
        let (_, ranges, offset) = narrowest?;
        Some(Prefilter::Ranges { ranges, offset })
    }

    pub fn find(&self, haystack: &[u8], at: usize, end: usize) -> Option<usize> {
        match self {
            Prefilter::Memmem(finder) => finder.find(&haystack[at..end]).map(|i| at + i),
            Prefilter::Bytes { needles, offset } => {
                let from = at + offset;
                if from >= end {
                    return None;
                }
                let window = &haystack[from..end];
                let found = match needles.len() {
                    1 => memchr::memchr(needles[0], window),
                    2 => memchr::memchr2(needles[0], needles[1], window),
                    _ => memchr::memchr3(needles[0], needles[1], needles[2], window),
                }?;
                Some(at + found)
            }
            Prefilter::Ranges { ranges, offset } => {
                let from = at + offset;
                if from >= end {
                    return None;
                }
                find_in_ranges(&haystack[from..end], ranges).map(|found| at + found)
            }
            Prefilter::Packed(packed) => packed.find(&haystack[..end], at),
            Prefilter::Teddy(teddy) => teddy
                .find(&haystack[..end], at)
                .map(|(candidate, _)| candidate),
            Prefilter::NonAscii => find_non_ascii(&haystack[at..end]).map(|found| at + found),
            Prefilter::LineStart(terminator) => {
                if at == 0 || haystack[at - 1] == *terminator {
                    return Some(at);
                }
                memchr::memchr(*terminator, &haystack[at..end]).map(|found| at + found + 1)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct LiteralSet {
    literals: Vec<Vec<u8>>,
    candidates: Prefilter,
    allowed: Vec<[bool; 256]>,
}

impl LiteralSet {
    pub fn new(literals: &[Vec<u8>]) -> Option<LiteralSet> {
        assert!(
            literals.iter().all(|l| !l.is_empty()),
            "a literal set holds no empty literal"
        );
        let candidates = Prefilter::from_literals(literals)?;
        let shortest = literals.iter().map(Vec::len).min()?;
        let mut allowed = vec![[false; 256]; shortest];
        for literal in literals {
            for (offset, &b) in literal[..shortest].iter().enumerate() {
                allowed[offset][usize::from(b)] = true;
            }
        }
        Some(LiteralSet {
            literals: literals.to_vec(),
            candidates,
            allowed,
        })
    }

    fn matches_at(&self, haystack: &[u8], candidate: usize) -> Option<(usize, usize)> {
        let rest = &haystack[candidate..];
        if rest.len() < self.allowed.len()
            || !self
                .allowed
                .iter()
                .zip(rest)
                .all(|(set, &b)| set[usize::from(b)])
        {
            return None;
        }
        let literal = self
            .literals
            .iter()
            .find(|literal| rest.starts_with(literal))?;
        Some((candidate, candidate + literal.len()))
    }

    pub fn find(&self, haystack: &[u8], start: usize) -> Option<(usize, usize)> {
        if let Prefilter::Teddy(teddy) = &self.candidates {
            let (candidate, verified) = teddy.find(haystack, start)?;
            if let Verified::One(len) = verified {
                return Some((candidate, candidate + len));
            }
            let found = self.matches_at(haystack, candidate);
            assert!(found.is_some(), "a verified candidate starts a literal");
            return found;
        }
        if let Prefilter::Bytes { needles, offset } = &self.candidates {
            let from = start + offset;
            if from >= haystack.len() {
                return None;
            }
            let window = &haystack[from..];
            return match needles.len() {
                1 => memchr::memchr_iter(needles[0], window)
                    .find_map(|i| self.matches_at(haystack, start + i)),
                2 => memchr::memchr2_iter(needles[0], needles[1], window)
                    .find_map(|i| self.matches_at(haystack, start + i)),
                _ => memchr::memchr3_iter(needles[0], needles[1], needles[2], window)
                    .find_map(|i| self.matches_at(haystack, start + i)),
            };
        }
        let mut at = start;
        loop {
            let candidate = self.candidates.find(haystack, at, haystack.len())?;
            if let Some(found) = self.matches_at(haystack, candidate) {
                return Some(found);
            }
            at = candidate + 1;
        }
    }
}

pub fn accelerates(hir: &Hir) -> bool {
    usable(&prefixes(hir))
        .is_some_and(|literals| Prefilter::from_literals_with(&literals, false).is_some())
}

pub fn prefix_prefilter(hir: &Hir) -> Option<Prefilter> {
    let literals = usable(&prefixes(hir))?;
    Prefilter::from_literals(&literals)
}

fn required(hir: &Hir, best: &mut Option<(usize, Vec<Vec<u8>>)>) {
    if let Some(literals) = usable(&prefixes(hir)) {
        let value = score(&literals);
        if best.as_ref().is_none_or(|(s, _)| value > *s) {
            *best = Some((value, literals));
        }
    }
    match hir.kind() {
        HirKind::Capture(capture) => required(&capture.sub, best),
        HirKind::Repetition(repetition) if repetition.min > 0 => required(&repetition.sub, best),
        HirKind::Concat(subs) => {
            for sub in subs {
                required(sub, best);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Debug)]
pub struct Required {
    prefilter: Prefilter,
    literals: Vec<Vec<u8>>,
}

impl Required {
    pub fn find(&self, haystack: &[u8]) -> Option<usize> {
        let mut at = 0;
        loop {
            let found = self.prefilter.find(haystack, at, haystack.len())?;
            if self.literals.len() == 1
                || self
                    .literals
                    .iter()
                    .any(|l| haystack[found..].starts_with(l))
            {
                return Some(found);
            }
            at = found + 1;
        }
    }
}

pub fn required_literals(hir: &Hir) -> Option<Required> {
    let mut best = None;
    required(hir, &mut best);
    let (_, literals) = best?;
    Prefilter::from_literals_with(&literals, false)?;
    let prefilter = Prefilter::from_literals(&literals)?;
    Some(Required {
        prefilter,
        literals,
    })
}

pub fn pure_literal(hir: &Hir) -> Option<Vec<u8>> {
    match hir.kind() {
        HirKind::Literal(bytes) => Some(bytes.to_vec()),
        _ => None,
    }
}
