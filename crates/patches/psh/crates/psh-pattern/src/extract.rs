use alloc::vec;
use alloc::vec::Vec;

use crate::hir::{Class, Hir, HirKind, Look, Repetition};

const CLASS_LIMIT_CHARACTERS: usize = 10;
const REPEAT_LIMIT: u32 = 10;
const LITERAL_LIMIT_BYTES: usize = 100;
const TOTAL_LIMIT_LITERALS: usize = 250;
const ALTERNATION_BYPASS_MINIMUM_LITERALS: usize = 3000;
const COMMON_BYTE_RANK: u8 = 200;
const POISONOUS_BYTE_RANK: u8 = 250;
const FAST_EXACT_LITERALS_MAX: usize = 16;
const SHRINK_ATTEMPTS: [(usize, usize); 5] = [(5, 10), (4, 10), (3, 64), (2, 64), (1, 10)];
const SHRUNK_LITERALS_MAX: usize = 64;

const BYTE_RANK: [u8; 256] = [
    55, 52, 51, 50, 49, 48, 47, 46, 45, 103, 242, 66, 67, 229, 44, 43, 42, 41, 40, 39, 38, 37, 36,
    35, 34, 33, 56, 32, 31, 30, 29, 28, 255, 148, 164, 149, 136, 160, 155, 173, 221, 222, 134, 122,
    232, 202, 215, 224, 208, 220, 204, 187, 183, 179, 177, 168, 178, 200, 226, 195, 154, 184, 174,
    126, 120, 191, 157, 194, 170, 189, 162, 161, 150, 193, 142, 137, 171, 176, 185, 167, 186, 112,
    175, 192, 188, 156, 140, 143, 123, 133, 128, 147, 138, 146, 114, 223, 151, 249, 216, 238, 236,
    253, 227, 218, 230, 247, 135, 180, 241, 233, 246, 244, 231, 139, 245, 243, 251, 235, 201, 196,
    240, 214, 152, 182, 205, 181, 127, 27, 212, 211, 210, 213, 228, 197, 169, 159, 131, 172, 105,
    80, 98, 96, 97, 81, 207, 145, 116, 115, 144, 130, 153, 121, 107, 132, 109, 110, 124, 111, 82,
    108, 118, 141, 113, 129, 119, 125, 165, 117, 92, 106, 83, 72, 99, 93, 65, 79, 166, 237, 163,
    199, 190, 225, 209, 203, 198, 217, 219, 206, 234, 248, 158, 239, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255,
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct Literal {
    bytes: Vec<u8>,
    exact: bool,
}

impl Literal {
    fn exact(bytes: Vec<u8>) -> Literal {
        Literal { bytes, exact: true }
    }

    fn keep_first_bytes(&mut self, length: usize) {
        if length >= self.bytes.len() {
            return;
        }
        self.exact = false;
        self.bytes.truncate(length);
    }

    fn is_poisonous(&self) -> bool {
        self.bytes.is_empty()
            || (self.bytes.len() == 1
                && BYTE_RANK[usize::from(self.bytes[0])] >= POISONOUS_BYTE_RANK)
    }
}

#[derive(Clone, Debug)]
struct Seq {
    literals: Option<Vec<Literal>>,
}

impl Seq {
    fn empty() -> Seq {
        Seq {
            literals: Some(Vec::new()),
        }
    }

    fn infinite() -> Seq {
        Seq { literals: None }
    }

    fn singleton(literal: Literal) -> Seq {
        Seq {
            literals: Some(vec![literal]),
        }
    }

    fn len(&self) -> Option<usize> {
        self.literals.as_ref().map(Vec::len)
    }

    fn push(&mut self, literal: Literal) {
        let Some(literals) = &mut self.literals else {
            return;
        };
        if literals.last() == Some(&literal) {
            return;
        }
        literals.push(literal);
    }

    fn make_inexact(&mut self) {
        if let Some(literals) = &mut self.literals {
            literals
                .iter_mut()
                .for_each(|literal| literal.exact = false);
        }
    }

    fn is_exact(&self) -> bool {
        self.literals
            .as_ref()
            .is_some_and(|literals| literals.iter().all(|literal| literal.exact))
    }

    fn is_inexact(&self) -> bool {
        self.literals
            .as_ref()
            .is_none_or(|literals| literals.iter().all(|literal| !literal.exact))
    }

    fn min_literal_len(&self) -> Option<usize> {
        self.literals
            .as_ref()?
            .iter()
            .map(|literal| literal.bytes.len())
            .min()
    }

    fn keep_first_bytes(&mut self, length: usize) {
        if let Some(literals) = &mut self.literals {
            literals
                .iter_mut()
                .for_each(|literal| literal.keep_first_bytes(length));
        }
    }

    fn dedup(&mut self) {
        if let Some(literals) = &mut self.literals {
            literals.dedup_by(|second, first| {
                if second.bytes != first.bytes {
                    return false;
                }
                if second.exact != first.exact {
                    second.exact = false;
                    first.exact = false;
                }
                true
            });
        }
    }

    fn cross_forward(&mut self, other: &mut Seq) {
        let Some(right) = &mut other.literals else {
            if self.min_literal_len() == Some(0) {
                *self = Seq::infinite();
            } else {
                self.make_inexact();
            }
            return;
        };
        let Some(left) = &mut self.literals else {
            right.clear();
            return;
        };
        let mut crossed = Vec::with_capacity(left.len().saturating_mul(right.len()));
        for literal in left.drain(..) {
            if !literal.exact {
                crossed.push(literal);
                continue;
            }
            for suffix in right.iter() {
                let mut bytes = Vec::with_capacity(literal.bytes.len() + suffix.bytes.len());
                bytes.extend_from_slice(&literal.bytes);
                bytes.extend_from_slice(&suffix.bytes);
                crossed.push(Literal {
                    bytes,
                    exact: suffix.exact,
                });
            }
        }
        *left = crossed;
        right.clear();
        self.dedup();
    }

    fn union(&mut self, other: &mut Seq) {
        let Some(right) = &mut other.literals else {
            self.literals = None;
            return;
        };
        let Some(left) = &mut self.literals else {
            return;
        };
        left.append(right);
        self.dedup();
    }

    fn longest_common_prefix(&self) -> Option<&[u8]> {
        let literals = self.literals.as_ref()?;
        let base = &literals.first()?.bytes;
        let mut length = base.len();
        for literal in &literals[1..] {
            length = literal
                .bytes
                .iter()
                .zip(base[..length].iter())
                .take_while(|(a, b)| a == b)
                .count();
            if length == 0 {
                return Some(&[]);
            }
        }
        Some(&base[..length])
    }

    fn optimize_for_prefix_by_preference(&mut self) {
        let Some(original_len) = self.len() else {
            return;
        };
        if self.min_literal_len() == Some(0) {
            self.literals = None;
            return;
        }
        if let Some(literals) = &mut self.literals {
            minimize_by_preference(literals);
        }
        if let Some(prefix) = self.longest_common_prefix() {
            let prefix_len = prefix.len();
            if original_len > 1
                && (1..=3).contains(&prefix_len)
                && BYTE_RANK[usize::from(prefix[0])] < COMMON_BYTE_RANK
            {
                self.keep_first_bytes(1);
                self.dedup();
                return;
            }
            let fast =
                self.is_exact() && self.len().is_some_and(|len| len <= FAST_EXACT_LITERALS_MAX);
            if prefix_len > 4 || (prefix_len > 1 && !fast) {
                self.keep_first_bytes(prefix_len);
                self.dedup();
                assert_eq!(
                    self.len(),
                    Some(1),
                    "literals sharing a whole prefix collapse into one"
                );
            }
        }
        let exact = if self.is_exact() {
            Some(self.clone())
        } else {
            None
        };
        for (keep, limit) in SHRINK_ATTEMPTS {
            let Some(len) = self.len() else { break };
            if len <= limit {
                break;
            }
            self.keep_first_bytes(keep);
            if let Some(literals) = &mut self.literals {
                minimize_by_preference(literals);
            }
        }
        if self
            .literals
            .as_ref()
            .is_some_and(|literals| literals.iter().any(Literal::is_poisonous))
        {
            self.literals = None;
        }
        if let Some(exact) = exact
            && (self.literals.is_none()
                || self.min_literal_len().is_none_or(|len| len <= 2)
                || self.len().is_none_or(|len| len > SHRUNK_LITERALS_MAX))
        {
            *self = exact;
        }
    }
}

fn minimize_by_preference(literals: &mut Vec<Literal>) {
    let mut kept: Vec<Literal> = Vec::with_capacity(literals.len());
    for literal in literals.drain(..) {
        if kept
            .iter()
            .any(|earlier| literal.bytes.starts_with(&earlier.bytes))
        {
            continue;
        }
        kept.push(literal);
    }
    *literals = kept;
}

fn class_over_limit(lengths: impl Iterator<Item = usize>) -> bool {
    let mut count = 0usize;
    for length in lengths {
        if count > CLASS_LIMIT_CHARACTERS {
            return true;
        }
        count += length;
    }
    count > CLASS_LIMIT_CHARACTERS
}

fn enforce_literal_len(seq: &mut Seq) {
    seq.keep_first_bytes(LITERAL_LIMIT_BYTES);
}

fn cross(mut first: Seq, second: &mut Seq) -> Seq {
    if let (Some(a), Some(b)) = (first.len(), second.len())
        && a.saturating_mul(b) > TOTAL_LIMIT_LITERALS
    {
        second.literals = None;
    }
    first.cross_forward(second);
    assert!(
        first.len().is_none_or(|len| len <= TOTAL_LIMIT_LITERALS),
        "a crossed sequence stays within its limit"
    );
    enforce_literal_len(&mut first);
    first
}

fn union(mut first: Seq, second: &mut Seq) -> Seq {
    let over = |a: &Seq, b: &Seq| matches!((a.len(), b.len()), (Some(x), Some(y)) if x.saturating_add(y) > TOTAL_LIMIT_LITERALS);
    if over(&first, second) {
        first.keep_first_bytes(4);
        second.keep_first_bytes(4);
        first.dedup();
        second.dedup();
        if over(&first, second) {
            second.literals = None;
        }
    }
    first.union(second);
    assert!(
        first.len().is_none_or(|len| len <= TOTAL_LIMIT_LITERALS),
        "a union stays within its limit"
    );
    first
}

fn extract(hir: &Hir) -> Seq {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => Seq::singleton(Literal::exact(Vec::new())),
        HirKind::Literal(bytes) => {
            let mut seq = Seq::singleton(Literal::exact(bytes.to_vec()));
            enforce_literal_len(&mut seq);
            seq
        }
        HirKind::Class(Class::Unicode(class)) => {
            if class_over_limit(class.ranges().iter().map(|range| range.len())) {
                return Seq::infinite();
            }
            let mut seq = Seq::empty();
            for range in class.ranges() {
                for value in range.start..=range.end {
                    let Some(c) = char::from_u32(value) else {
                        continue;
                    };
                    let mut buffer = [0; 4];
                    seq.push(Literal::exact(
                        c.encode_utf8(&mut buffer).as_bytes().to_vec(),
                    ));
                }
            }
            enforce_literal_len(&mut seq);
            seq
        }
        HirKind::Class(Class::Bytes(class)) => {
            if class_over_limit(class.ranges().iter().map(|range| range.len())) {
                return Seq::infinite();
            }
            let mut seq = Seq::empty();
            for range in class.ranges() {
                for value in range.start..=range.end {
                    seq.push(Literal::exact(vec![
                        u8::try_from(value).expect("byte class ranges hold bytes"),
                    ]));
                }
            }
            enforce_literal_len(&mut seq);
            seq
        }
        HirKind::Repetition(repetition) => extract_repetition(repetition),
        HirKind::Capture(capture) => extract(&capture.sub),
        HirKind::Concat(subs) => {
            let mut seq = Seq::singleton(Literal::exact(Vec::new()));
            for sub in subs {
                if seq.is_inexact() {
                    break;
                }
                seq = cross(seq, &mut extract(sub));
            }
            seq
        }
        HirKind::Alternation(subs) => {
            let mut seq = Seq::empty();
            for sub in subs {
                if seq.literals.is_none() {
                    break;
                }
                seq = union(seq, &mut extract(sub));
            }
            seq
        }
    }
}

fn extract_repetition(repetition: &Repetition) -> Seq {
    let mut sub = extract(&repetition.sub);
    if repetition.min == 0 {
        if repetition.max != Some(1) {
            sub.make_inexact();
        }
        let mut empty = Seq::singleton(Literal::exact(Vec::new()));
        if !repetition.greedy {
            core::mem::swap(&mut sub, &mut empty);
        }
        return union(sub, &mut empty);
    }
    let mut seq = Seq::singleton(Literal::exact(Vec::new()));
    for _ in 0..repetition.min.min(REPEAT_LIMIT) {
        if seq.is_inexact() {
            break;
        }
        seq = cross(seq, &mut sub.clone());
    }
    if repetition.max != Some(repetition.min) || repetition.min > REPEAT_LIMIT {
        seq.make_inexact();
    }
    seq
}

fn alternation_literals(hir: &Hir) -> Option<Vec<Vec<u8>>> {
    if !hir.properties().alternation_literal {
        return None;
    }
    let HirKind::Alternation(subs) = hir.kind() else {
        return None;
    };
    if subs.len() < ALTERNATION_BYPASS_MINIMUM_LITERALS {
        return None;
    }
    subs.iter()
        .map(|sub| match sub.kind() {
            HirKind::Literal(bytes) => Some(bytes.to_vec()),
            _ => None,
        })
        .collect()
}

pub fn bypass_literals(hir: &Hir) -> Option<Vec<Vec<u8>>> {
    let properties = hir.properties();
    if properties.look_set_prefix.contains(Look::Start)
        || properties.explicit_captures_len != 0
        || !properties.look_set.is_empty()
    {
        return None;
    }
    let mut prefixes = Seq::empty();
    prefixes.union(&mut extract(hir));
    prefixes.optimize_for_prefix_by_preference();
    if prefixes.is_exact()
        && let Some(literals) = prefixes.literals
        && !literals.is_empty()
        && literals.iter().all(|literal| !literal.bytes.is_empty())
    {
        return Some(literals.into_iter().map(|literal| literal.bytes).collect());
    }
    alternation_literals(hir)
}
