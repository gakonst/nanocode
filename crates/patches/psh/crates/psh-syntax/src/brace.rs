//! Brace expansion structure: `{a,b}` and `{1..9}` found in the unquoted literal text of
//! a word. Bash expands braces before anything else and only where the braces and commas
//! are unquoted, so the structure is fixed at parse time.

use alloc::vec::Vec;

use crate::ast::{Brace, Part, Word, WordPart};
use crate::span::Span;

enum Atom {
    Open(u32),
    Close(u32),
    Comma(u32),
    Text(Span),
    Part(WordPart),
}

impl Atom {
    fn span(&self) -> Span {
        match self {
            Atom::Open(p) | Atom::Close(p) | Atom::Comma(p) => Span::new(*p, *p + 1),
            Atom::Text(s) => *s,
            Atom::Part(p) => p.span,
        }
    }
}

/// Rewrites `parts` so that brace expansions become [`Part::Brace`] parts.
pub(crate) fn brace_pass(src: &[u8], parts: Vec<WordPart>) -> Vec<WordPart> {
    let has_open = parts
        .iter()
        .any(|p| matches!(p.kind, Part::Lit) && src[p.span.range()].contains(&b'{'));
    if !has_open {
        return parts;
    }
    let mut atoms = Vec::new();
    for part in parts {
        if !matches!(part.kind, Part::Lit) {
            atoms.push(Atom::Part(part));
            continue;
        }
        let mut run = part.span.start;
        for i in part.span.start..part.span.end {
            let marker = match src[i as usize] {
                b'{' => Atom::Open(i),
                b'}' => Atom::Close(i),
                b',' => Atom::Comma(i),
                _ => continue,
            };
            if run < i {
                atoms.push(Atom::Text(Span::new(run, i)));
            }
            atoms.push(marker);
            run = i + 1;
        }
        if run < part.span.end {
            atoms.push(Atom::Text(Span::new(run, part.span.end)));
        }
    }
    build(src, atoms)
}

fn push_lit(out: &mut Vec<WordPart>, span: Span) {
    if let Some(last) = out.last_mut()
        && matches!(last.kind, Part::Lit)
        && last.span.end == span.start
    {
        last.span.end = span.end;
        return;
    }
    out.push(WordPart {
        span,
        kind: Part::Lit,
    });
}

fn build(src: &[u8], atoms: Vec<Atom>) -> Vec<WordPart> {
    let mut out = Vec::new();
    let mut atoms: Vec<Option<Atom>> = atoms.into_iter().map(Some).collect();
    let mut i = 0;
    while i < atoms.len() {
        if matches!(atoms[i], Some(Atom::Open(_)))
            && let Some((part, next)) = try_brace(src, &mut atoms, i)
        {
            out.push(part);
            i = next;
            continue;
        }
        match atoms[i].take().expect("atom used once") {
            Atom::Part(p) => out.push(p),
            a => push_lit(&mut out, a.span()),
        }
        i += 1;
    }
    out
}

fn try_brace(src: &[u8], atoms: &mut [Option<Atom>], open: usize) -> Option<(WordPart, usize)> {
    let mut depth = 0usize;
    let mut commas = Vec::new();
    let mut close = None;
    for (j, atom) in atoms.iter().enumerate().skip(open) {
        match atom.as_ref()? {
            Atom::Open(_) => depth += 1,
            Atom::Close(_) => {
                depth -= 1;
                if depth == 0 {
                    close = Some(j);
                    break;
                }
            }
            Atom::Comma(_) if depth == 1 => commas.push(j),
            _ => {}
        }
    }
    let close = close?;
    let span = atoms[open]
        .as_ref()?
        .span()
        .to(atoms[close].as_ref()?.span());
    if commas.is_empty() {
        // `{x..y}`: the inside must be one plain text run.
        if close != open + 2 {
            return None;
        }
        let Some(Atom::Text(text)) = &atoms[open + 1] else {
            return None;
        };
        let (start, end, step) = seq_parts(src, *text)?;
        for a in &mut atoms[open..=close] {
            *a = None;
        }
        return Some((
            WordPart {
                span,
                kind: Part::Brace(alloc::boxed::Box::new(Brace::Seq { start, end, step })),
            },
            close + 1,
        ));
    }
    let mut alts = Vec::new();
    let mut bounds = Vec::new();
    bounds.push(open);
    bounds.extend(commas.iter().copied());
    bounds.push(close);
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        let from = atoms[a].as_ref()?.span().end;
        let to = atoms[b].as_ref()?.span().start;
        let inner: Vec<Atom> = atoms[a + 1..b]
            .iter_mut()
            .map(|x| x.take().unwrap())
            .collect();
        alts.push(Word {
            span: Span::new(from, to),
            parts: build(src, inner),
        });
    }
    for a in &mut atoms[open..=close] {
        *a = None;
    }
    Some((
        WordPart {
            span,
            kind: Part::Brace(alloc::boxed::Box::new(Brace::Alts(alts))),
        },
        close + 1,
    ))
}

/// Splits `x..y[..step]` into its pieces if it is a valid sequence expression.
fn seq_parts(src: &[u8], text: Span) -> Option<(Span, Span, Option<Span>)> {
    let bytes = &src[text.range()];
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'.' && bytes[i + 1] == b'.' {
            pieces.push((start, i));
            i += 2;
            start = i;
        } else {
            i += 1;
        }
    }
    pieces.push((start, bytes.len()));
    if pieces.len() != 2 && pieces.len() != 3 {
        return None;
    }
    let sp = |(a, b): (usize, usize)| Span::new(text.start + a as u32, text.start + b as u32);
    let piece = |(a, b): (usize, usize)| &bytes[a..b];
    let (x, y) = (piece(pieces[0]), piece(pieces[1]));
    let ok = (is_int(x) && is_int(y)) || (is_char(x) && is_char(y));
    if !ok {
        return None;
    }
    let step = match pieces.get(2) {
        Some(&p) if is_int(piece(p)) => Some(sp(p)),
        Some(_) => return None,
        None => None,
    };
    Some((sp(pieces[0]), sp(pieces[1]), step))
}

fn is_int(b: &[u8]) -> bool {
    let digits = match b.first() {
        Some(b'-' | b'+') => &b[1..],
        _ => b,
    };
    !digits.is_empty() && digits.iter().all(u8::is_ascii_digit)
}

fn is_char(b: &[u8]) -> bool {
    b.len() == 1 && !b[0].is_ascii_digit()
}
