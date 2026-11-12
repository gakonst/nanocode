//! `tr [-cdst] SET1 [SET2]` (`-C` and `-u` from BSD): ranges, `[:class:]`, `[=c=]`,
//! `[c*n]`, backslash and octal escapes. Written for psh after GNU's documented
//! behavior.
//!
//! Sets of ASCII characters and escapes work on bytes, exactly as GNU's `tr` (and BSD's
//! in the C locale). A set that contains a non-ASCII character works on UTF-8
//! characters instead, as BSD's does in a UTF-8 locale: `tr 'é' 'e'` means the
//! character, where GNU would translate its two bytes separately (D1162). Classes are
//! the C locale's (ASCII) in both modes.

use std::collections::{HashMap, HashSet};

use psh_engine::util::UtilCtx;

use crate::common;
use crate::opts::{Spec, both, flag};
use crate::textio::decode_char;

pub const SPEC: Spec = Spec {
    name: "tr",
    opts: &[
        both('c', "complement"),
        flag('C'),
        both('d', "delete"),
        both('s', "squeeze-repeats"),
        both('t', "truncate-set1"),
        flag('u'),
    ],
    posix_order: false,
};

/// Raw bytes that are not part of valid UTF-8 (and octal escapes above 0x7f) in
/// character mode: past the last code point, so they never equal a character.
const RAW: u32 = 0x11_0000;

/// One element of a parsed set.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Unit(u32),
    Range(u32, u32),
    /// `[:name:]`
    Class(&'static str),
    /// `[c*n]`; `None` fills SET2 to the length of SET1.
    Repeat(u32, Option<usize>),
}

const CLASSES: &[&str] = &[
    "alnum", "alpha", "blank", "cntrl", "digit", "graph", "lower", "print", "punct", "space",
    "upper", "xdigit",
];

fn class_members(name: &str) -> Vec<u32> {
    (0u8..128)
        .filter(|&b| match name {
            "alnum" => b.is_ascii_alphanumeric(),
            "alpha" => b.is_ascii_alphabetic(),
            "blank" => b == b' ' || b == b'\t',
            "cntrl" => b.is_ascii_control(),
            "digit" => b.is_ascii_digit(),
            "graph" => b.is_ascii_graphic(),
            "lower" => b.is_ascii_lowercase(),
            "print" => b.is_ascii_graphic() || b == b' ',
            "punct" => b.is_ascii_punctuation(),
            "space" => b.is_ascii_whitespace() || b == 0x0b,
            "upper" => b.is_ascii_uppercase(),
            "xdigit" => b.is_ascii_hexdigit(),
            _ => false,
        })
        .map(u32::from)
        .collect()
}

/// Reads one character of a set at `i`: an escape or a literal. Returns the unit and
/// the length consumed.
fn read_unit(s: &[u8], i: usize, chars: bool) -> (u32, usize) {
    if s[i] == b'\\' && i + 1 < s.len() {
        let c = s[i + 1];
        if (b'0'..=b'7').contains(&c) {
            let mut v = 0u32;
            let mut n = 0;
            while n < 3 && i + 1 + n < s.len() && (b'0'..=b'7').contains(&s[i + 1 + n]) {
                let next = v * 8 + u32::from(s[i + 1 + n] - b'0');
                if next > 255 {
                    break;
                }
                v = next;
                n += 1;
            }
            let v = if chars && v >= 0x80 { RAW + v } else { v };
            return (v, 1 + n);
        }
        let v = match c {
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'n' => 10,
            b'r' => 13,
            b't' => 9,
            b'v' => 11,
            _ => return decode(&s[i + 1..], chars).map_or((u32::from(c), 2), |(u, n)| (u, n + 1)),
        };
        return (v, 2);
    }
    if s[i] == b'\\' {
        // A lone trailing backslash is itself.
        return (u32::from(b'\\'), 1);
    }
    decode(&s[i..], chars).unwrap_or((u32::from(s[i]), 1))
}

fn decode(s: &[u8], chars: bool) -> Option<(u32, usize)> {
    if !chars {
        return Some((u32::from(s[0]), 1));
    }
    match decode_char(s) {
        (Some(c), n) => Some((c as u32, n)),
        (None, _) => Some((RAW + u32::from(s[0]), 1)),
    }
}

/// `repeats`: whether `[c*n]` is a construct (SET2) or literal text (SET1).
fn parse_set(s: &[u8], chars: bool, repeats: bool, utf8: bool) -> Result<Vec<Item>, String> {
    let q = |t: &str| common::quote_in(utf8, t.as_bytes());
    let mut items = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'[' && i + 1 < s.len() {
            // [:class:]
            if s[i + 1] == b':'
                && let Some(end) = find(&s[i + 2..], b":]")
            {
                let name = String::from_utf8_lossy(&s[i + 2..i + 2 + end]).into_owned();
                let Some(&c) = CLASSES.iter().find(|&&c| c == name) else {
                    return Err(format!("invalid character class {}", q(&name)));
                };
                items.push(Item::Class(c));
                i += end + 4;
                continue;
            }
            // [=c=]
            if s[i + 1] == b'=' && i + 2 < s.len() {
                let (u, n) = read_unit(s, i + 2, chars);
                if s[i + 2 + n..].starts_with(b"=]") {
                    items.push(Item::Unit(u));
                    i += 2 + n + 2;
                    continue;
                }
            }
            // [c*n]
            if repeats {
                let (u, n) = read_unit(s, i + 1, chars);
                let star = i + 1 + n;
                if s.get(star) == Some(&b'*')
                    && let Some(close) = s[star + 1..].iter().position(|&b| b == b']')
                {
                    let count = &s[star + 1..star + 1 + close];
                    if count.iter().all(u8::is_ascii_digit) {
                        let text = String::from_utf8_lossy(count);
                        // A leading 0 makes the count octal; 0 or none fills.
                        let n = if count.is_empty() {
                            None
                        } else {
                            let v = if count[0] == b'0' {
                                usize::from_str_radix(&text, 8).ok()
                            } else {
                                text.parse().ok()
                            };
                            match v {
                                Some(0) => None,
                                Some(n) => Some(n),
                                None => {
                                    return Err(format!(
                                        "invalid repeat count {} in [c*n] construct",
                                        q(&text)
                                    ));
                                }
                            }
                        };
                        items.push(Item::Repeat(u, n));
                        i = star + 1 + close + 1;
                        continue;
                    }
                }
            }
        }
        let (u, n) = read_unit(s, i, chars);
        // a-z
        if s.get(i + n) == Some(&b'-') && i + n + 1 < s.len() {
            let (v, m) = read_unit(s, i + n + 1, chars);
            if v < u {
                let text = String::from_utf8_lossy(&s[i..i + n + 1 + m]).into_owned();
                return Err(format!(
                    "range-endpoints of '{text}' are in reverse collating sequence order"
                ));
            }
            items.push(Item::Range(u, v));
            i += n + 1 + m;
            continue;
        }
        items.push(Item::Unit(u));
        i += n;
    }
    Ok(items)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The units of a set, in order; `[c*]` fills to `fill`.
fn expand(items: &[Item], fill: usize) -> Vec<u32> {
    let fixed: usize = items
        .iter()
        .map(|it| match it {
            Item::Unit(_) => 1,
            Item::Range(a, b) => (b - a + 1) as usize,
            Item::Class(c) => class_members(c).len(),
            Item::Repeat(_, n) => n.unwrap_or(0),
        })
        .sum();
    let mut out = Vec::new();
    let mut filled = false;
    for it in items {
        match it {
            Item::Unit(u) => out.push(*u),
            Item::Range(a, b) => out.extend(*a..=*b),
            Item::Class(c) => out.extend(class_members(c)),
            Item::Repeat(u, Some(n)) => out.extend(std::iter::repeat_n(*u, *n)),
            Item::Repeat(u, None) => {
                if !filled {
                    filled = true;
                    out.extend(std::iter::repeat_n(*u, fill.saturating_sub(fixed)));
                }
            }
        }
    }
    out
}

/// What happens to each unit.
struct Plan {
    delete: HashSet<u32>,
    map: HashMap<u32, u32>,
    /// With `-c` while translating: everything outside SET1 maps by its position in the
    /// complement (`map_rest` holds SET2, padded by its last unit).
    complement_of: Option<HashSet<u32>>,
    complement_sorted: Vec<u32>,
    map_rest: Vec<u32>,
    squeeze: Option<SqueezeSet>,
}

enum SqueezeSet {
    In(HashSet<u32>),
    NotIn(HashSet<u32>),
}

impl SqueezeSet {
    fn contains(&self, u: u32) -> bool {
        match self {
            SqueezeSet::In(s) => s.contains(&u),
            SqueezeSet::NotIn(s) => !s.contains(&u),
        }
    }
}

impl Plan {
    fn translate(&self, u: u32) -> u32 {
        if let Some(set1) = &self.complement_of {
            if set1.contains(&u) {
                return u;
            }
            // The position of `u` among the units not in SET1.
            let below = self.complement_sorted.partition_point(|&x| x < u);
            let idx = u as usize - below;
            return self.map_rest[idx.min(self.map_rest.len() - 1)];
        }
        self.map.get(&u).copied().unwrap_or(u)
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let complement = p.has(&SPEC, "complement") || p.has(&SPEC, "C");
    let delete = p.has(&SPEC, "delete");
    let squeeze = p.has(&SPEC, "squeeze-repeats");
    let truncate = p.has(&SPEC, "truncate-set1");
    let ops = &p.operands;
    let wanted = if delete && squeeze || !delete && !squeeze {
        2
    } else {
        1
    };
    if ops.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    if ops.len() < wanted {
        let o = common::quote(ctx, &ops[0]);
        let why = if squeeze {
            "Two strings must be given when both deleting and squeezing repeats."
        } else {
            "Two strings must be given when translating."
        };
        return common::usage(ctx, &format!("missing operand after {o}\n{why}")).await;
    }
    if ops.len() > wanted && !(squeeze && !delete && ops.len() == 2) {
        let o = common::quote(ctx, &ops[wanted]);
        let mut msg = format!("extra operand {o}");
        if delete && ops.len() == 2 {
            msg.push_str("\nOnly one string may be given when deleting without squeezing repeats.");
        }
        return common::usage(ctx, &msg).await;
    }
    let chars = ops.iter().any(|o| o.iter().any(|&b| b >= 0x80));
    let set1_items = match parse_set(&ops[0], chars, false, ctx.utf8()) {
        Ok(s) => s,
        Err(e) => {
            ctx.error(&e).await;
            return 1;
        }
    };
    let mut set1 = expand(&set1_items, 0);
    let set2 = match ops.get(1) {
        Some(o) => {
            let items = match parse_set(o, chars, true, ctx.utf8()) {
                Ok(s) => s,
                Err(e) => {
                    ctx.error(&e).await;
                    return 1;
                }
            };
            let translating = !delete;
            if translating
                && items
                    .iter()
                    .any(|i| matches!(i, Item::Class(c) if *c != "upper" && *c != "lower"))
            {
                ctx.error(
                    "when translating, the only character classes that may appear in\nstring2 are 'upper' and 'lower'",
                )
                .await;
                return 1;
            }
            Some(expand(&items, set1.len()))
        }
        None => None,
    };
    let set1_lookup: HashSet<u32> = set1.iter().copied().collect();
    let mut plan = Plan {
        delete: HashSet::new(),
        map: HashMap::new(),
        complement_of: None,
        complement_sorted: Vec::new(),
        map_rest: Vec::new(),
        squeeze: None,
    };
    let translating = !delete && set2.is_some();
    if delete && !complement {
        plan.delete = set1_lookup.clone();
    }
    if translating {
        let mut set2 = set2.clone().unwrap_or_default();
        if set2.is_empty() && !(truncate || set1.is_empty()) {
            ctx.error("when not truncating set1, string2 must be non-empty")
                .await;
            return 1;
        }
        if complement {
            let mut sorted: Vec<u32> = set1_lookup.iter().copied().collect();
            sorted.sort_unstable();
            plan.complement_sorted = sorted;
            plan.complement_of = Some(set1_lookup.clone());
            if set2.is_empty() {
                plan.complement_of = None;
            }
            plan.map_rest = set2;
        } else {
            if truncate {
                set1.truncate(set2.len());
            } else if let Some(&last) = set2.last() {
                while set2.len() < set1.len() {
                    set2.push(last);
                }
            }
            for (a, b) in set1.iter().zip(&set2) {
                plan.map.insert(*a, *b);
            }
        }
    }
    if squeeze {
        let s = match (&set2, translating || delete) {
            (Some(s2), true) => SqueezeSet::In(s2.iter().copied().collect()),
            _ if complement => SqueezeSet::NotIn(set1_lookup.clone()),
            _ => SqueezeSet::In(set1_lookup.clone()),
        };
        plan.squeeze = Some(s);
    }
    let delete_complement = delete && complement;
    // Sets of single bytes: what becomes of each byte (deleted, or the byte it turns
    // into and whether its repeats are squeezed) is worked out once, not per byte read.
    let bytes: Option<Vec<Option<(u8, bool)>>> = (!chars).then(|| {
        (0..=u32::from(u8::MAX))
            .map(|u| {
                let deleted = if delete_complement {
                    !set1_lookup.contains(&u)
                } else {
                    plan.delete.contains(&u)
                };
                let t = if translating { plan.translate(u) } else { u };
                let squeezed = plan.squeeze.as_ref().is_some_and(|sq| sq.contains(t));
                (!deleted).then_some((t as u8, squeezed))
            })
            .collect()
    });

    let map: Option<Vec<u8>> = bytes
        .as_ref()
        .and_then(|t| t.iter().map(|e| e.map(|(t, _)| t)).collect());
    let mut buf = vec![0u8; 64 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    let mut last_out: Option<u32> = None;
    let mut out = Vec::with_capacity(64 * 1024);
    loop {
        let n = match ctx.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                ctx.error(&format!("read error: {}", psh_engine::host::error_text(&e)))
                    .await;
                return 1;
            }
        };
        let eof = n == 0;
        if let Some(bytes) = &bytes
            && plan.squeeze.is_none()
        {
            let data = if let Some(map) = &map {
                buf[..n].iter_mut().for_each(|b| *b = map[usize::from(*b)]);
                &buf[..n]
            } else {
                out.resize(n + 1, 0);
                let mut j = 0;
                for &b in &buf[..n] {
                    let (t, keep) = bytes[usize::from(b)].map_or((0, 0), |(t, _)| (t, 1));
                    out[j] = t;
                    j += keep;
                }
                &out[..j]
            };
            if !data.is_empty()
                && let Err(e) = ctx.write(data).await
            {
                return common::write_failed(ctx, &e).await;
            }
            if eof {
                break;
            }
            ctx.check().await;
            continue;
        }
        let mut data = std::mem::take(&mut pending);
        data.extend_from_slice(&buf[..n]);
        let mut i = 0;
        if let Some(bytes) = &bytes {
            for &b in &data {
                let Some((t, squeezed)) = bytes[usize::from(b)] else {
                    continue;
                };
                if squeezed && last_out == Some(u32::from(t)) {
                    continue;
                }
                last_out = Some(u32::from(t));
                out.push(t);
            }
            i = data.len();
        }
        while i < data.len() {
            let (u, len) = if chars {
                let b = data[i];
                if b >= 0x80 && !eof {
                    let need = match b {
                        0xc2..=0xdf => 2,
                        0xe0..=0xef => 3,
                        0xf0..=0xf4 => 4,
                        _ => 1,
                    };
                    if i + need > data.len() {
                        pending = data[i..].to_vec();
                        break;
                    }
                }
                match decode_char(&data[i..]) {
                    (Some(c), n) => (c as u32, n),
                    (None, _) => (RAW + u32::from(data[i]), 1),
                }
            } else {
                (u32::from(data[i]), 1)
            };
            i += len;
            let deleted = if delete_complement {
                !set1_lookup.contains(&u)
            } else {
                plan.delete.contains(&u)
            };
            if deleted {
                continue;
            }
            let t = if translating { plan.translate(u) } else { u };
            if let Some(sq) = &plan.squeeze
                && last_out == Some(t)
                && sq.contains(t)
            {
                continue;
            }
            last_out = Some(t);
            push_unit(&mut out, t, chars);
        }
        if !out.is_empty() {
            if let Err(e) = ctx.write(&out).await {
                return common::write_failed(ctx, &e).await;
            }
            out.clear();
        }
        if eof {
            break;
        }
        ctx.check().await;
    }
    0
}

fn push_unit(out: &mut Vec<u8>, u: u32, chars: bool) {
    if !chars || u < 0x80 {
        out.push(u as u8);
    } else if u >= RAW {
        out.push((u - RAW) as u8);
    } else if let Some(c) = char::from_u32(u) {
        let mut b = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(s: &str) -> Vec<u32> {
        expand(&parse_set(s.as_bytes(), false, true, false).unwrap(), 0)
    }

    #[test]
    fn sets() {
        assert_eq!(set("a-c"), vec![97, 98, 99]);
        assert_eq!(set("\\n\\t\\101"), vec![10, 9, 65]);
        assert_eq!(set("[:digit:]").len(), 10);
        assert_eq!(set("[=a=]x"), vec![97, 120]);
        assert_eq!(
            expand(&parse_set(b"[x*]y", false, true, false).unwrap(), 4),
            vec![120, 120, 120, 121]
        );
        assert_eq!(
            expand(&parse_set(b"[x*3]", false, true, false).unwrap(), 0),
            vec![120; 3]
        );
        assert!(parse_set(b"z-a", false, true, false).is_err());
        assert!(parse_set(b"[:nope:]", false, true, false).is_err());
        assert_eq!(set("a-"), vec![97, 45]);
        assert_eq!(set("\\\\"), vec![92]);
    }
}
