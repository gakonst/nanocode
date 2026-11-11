//! Continuations whose layout a re-dump makes again (D2913).
//!
//! Origin tracking reads a continuation `K`'s frame as one of its origin `f` through K's
//! layout: where K copies `f`'s text (`segs`), where its own constructs are (`scaf`) and
//! what stands for `f`'s temporaries (`cmap`). K's record does not hold that layout; it
//! holds what makes it again from `f`: the site (`at`), the decisions the writer took on
//! values that shape K (`r`, the tape: which operands K reads again, a completion's kind,
//! …) and where K's text has generated text that depends on values (`h`, the holes).
//!
//! K is written as pieces: generated text (runs), copies of `f`'s text, marks of K's own
//! constructs and literals standing for temporaries, a run between any two others. The
//! writer writes K, then writes it again from a canonical activation: the site and its
//! slot names, every value a placeholder, with the tape. The two must have the same
//! pieces but for the texts of runs and literals; those that differ are the holes,
//! `[piece, length, ...]`. A re-dump writes the canonical K again, checks its fixed text
//! against K's own text, takes the holes' texts from it and so has the writer's pieces,
//! whose layout is the writer's.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::super::plain::{Act, BDecl, Ctx, KText, Slot, Using};
use super::{G, P, R, join, run};
use crate::buf::PushOut;
use crate::compiler::{Site, Special, Syntax};

/// A value of the canonical activation.
pub(in super::super) const PH: &str = "\u{1}";

/// A piece of a continuation's text.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Piece {
    Run(String),
    /// The original's text `[a, b)`.
    Copy(u32, u32),
    /// Where scaffold record `scaf[i]` is.
    Mark(usize),
    /// A temporary's literal, `cmap[i]`.
    Lit(usize, String),
}

/// A continuation as pieces, with its records.
pub(super) struct Out {
    pub pieces: Vec<Piece>,
    /// Kind, offset in the original, offset in K (set by [`layout`]), the rest of the
    /// record (JSON).
    pub scaf: Vec<(&'static str, u32, u32, String)>,
    /// A temporary of the original: slot, K's binding or the literal, whether literal.
    pub cmap: Vec<(String, String, bool)>,
    pub used: Vec<String>,
    pub args: Vec<String>,
    /// The names K binds at its top (D3001).
    pub top: Vec<String>,
    pub tape: Vec<u32>,
    /// Temporaries whose literal is another's: `(cmap index, the other's)`.
    pub aliases: Vec<(usize, usize)>,
    /// A method's home object's prototype: K's text goes in `({ __proto__: p, K }).K`.
    pub wrap: Option<String>,
}

impl G<'_, '_> {
    /// The continuation's pieces: `parts` rendered (no blank lines of K's own), K's
    /// helpers declared where the marker is.
    pub(super) fn flatten(self, parts: P) -> Out {
        let mut r = Render {
            ftext: &self.act.ftext,
            pieces: Vec::new(),
            run: String::new(),
            nl: false,
            helpers: None,
        };
        r.walk(&parts);
        let mut vals = Vec::new();
        vals_of(&parts, &mut vals);
        let tail = core::mem::take(&mut r.run);
        r.pieces.push_out(Piece::Run(tail));
        let mut text = String::new();
        for p in &r.pieces {
            match p {
                Piece::Run(t) | Piece::Lit(_, t) => text.push_str(t),
                Piece::Copy(a, b) => {
                    text.push_str(r.ftext.get(*a as usize..*b as usize).unwrap_or(""))
                }
                Piece::Mark(_) => {}
            }
        }
        // K's helpers, captured where the marker is.
        let act = self.act;
        const HELPERS: [&str; 17] = [
            "R", "G", "C", "D", "A", "L", "N", "Q", "X", "O", "I", "E", "P", "S", "Y", "U", "DD",
        ];
        // The calls `$djs_H(` of each helper, in the text and in its values: one pass each
        // (0215: a search per helper and value was most of this function).
        let calls = |t: &str, counts: &mut [usize; 17]| {
            for at in super::super::plain::dollar_indices(t, "$djs_") {
                let rest = &t.as_bytes()[at + 5..];
                // (Helper names are one or two letters.)
                let end = rest.iter().take(3).position(|&c| c == b'(');
                if let Some(end) = end
                    && let Some(h) = HELPERS.iter().position(|h| h.as_bytes() == &rest[..end])
                {
                    counts[h] += 1;
                }
            }
        };
        let mut in_text = [0usize; 17];
        calls(&text, &mut in_text);
        let mut in_vals = [0usize; 17];
        for v in &vals {
            calls(v, &mut in_vals);
        }
        let names: Vec<&str> = HELPERS
            .iter()
            .enumerate()
            // (Not those of a value written in K's text: a continuation written in place
            // captures its own, D3001.)
            .filter(|&(i, _)| in_text[i] > in_vals[i])
            .map(|(_, h)| *h)
            // A continuation of a continuation declares them already (its own
            // first line, restored from its slot).
            .filter(|h| !act.slots.iter().any(|s| s.name == cat!("$djs_", *h)))
            .collect();
        let mut decl = String::new();
        if !names.is_empty() {
            let list: Vec<String> = names
                .iter()
                .map(|h| cat!("$djs_", *h, " = $djs.", *h))
                .collect();
            decl = cat!("const ", join(&list, ", "), ";\n");
        }
        if self.uses_arguments && !text.contains("var $djs_arguments = ") {
            decl.push_str("const $djs_arguments = undefined;\n");
        }
        let mut pieces = r.pieces;
        if let Some((i, at)) = r.helpers
            && let Some(Piece::Run(t)) = pieces.get_mut(i)
        {
            t.insert_str(at, &decl);
        }
        Out {
            pieces,
            scaf: self.scaf,
            cmap: self.cmap,
            used: self.used,
            args: Vec::new(),
            top: Vec::new(),
            tape: self.tape,
            aliases: self.aliases,
            wrap: None,
        }
    }
}

/// The values `p` writes in K's text ([`P::Val`]).
fn vals_of<'p>(p: &'p P, out: &mut Vec<&'p str>) {
    match p {
        P::Val(t, _) => out.push_out(t.as_str()),
        P::L(list) => {
            for q in list {
                vals_of(q, out);
            }
        }
        P::Scope(_, _, q) => vals_of(q, out),
        _ => {}
    }
}

struct Render<'t> {
    ftext: &'t str,
    pieces: Vec<Piece>,
    run: String,
    /// The text so far ends a line.
    nl: bool,
    /// Where the helpers go: the piece (the run being written) and the offset in it.
    helpers: Option<(usize, usize)>,
}

impl Render<'_> {
    fn text(&mut self, t: &str) {
        let t = if self.nl {
            t.trim_start_matches('\n')
        } else {
            t
        };
        if !t.is_empty() {
            self.run.push_str(t);
            self.nl = t.ends_with('\n');
        }
    }

    fn item(&mut self, p: Piece, text: &str) {
        let run = core::mem::take(&mut self.run);
        self.pieces.push_out(Piece::Run(run));
        self.pieces.push_out(p);
        if !text.is_empty() {
            self.nl = text.ends_with('\n');
        }
    }

    fn walk(&mut self, p: &P) {
        match p {
            P::S(t) | P::Decl(_, t) | P::Val(t, _) => self.text(t),
            P::T(t) => self.text(t),
            P::Scope(names, braced, q) => {
                let braces = *braced && !names.is_empty();
                if braces {
                    self.text("{\n");
                }
                self.walk(q);
                if braces {
                    self.text("\n}\n");
                }
            }
            P::V(a, b) => {
                if b > a {
                    let t = self.ftext.get(*a as usize..*b as usize).unwrap_or("");
                    self.item(Piece::Copy(*a, *b), t);
                }
            }
            P::Lit(i, t) => self.item(Piece::Lit(*i, t.clone()), t),
            P::Mark(i) => self.item(Piece::Mark(*i), ""),
            P::Helpers => self.helpers = Some((self.pieces.len(), self.run.len())),
            P::L(list) => {
                for q in list {
                    self.walk(q);
                }
            }
        }
    }
}

/// The text of `pieces` and its segments `[kStart, fStart, len]`; sets the scaffold
/// records' offsets in K and the literals' texts in `cmap`.
pub(super) fn layout(
    pieces: &[Piece],
    ftext: &str,
    scaf: &mut [(&'static str, u32, u32, String)],
    cmap: &mut [(String, String, bool)],
    aliases: &[(usize, usize)],
) -> (String, Vec<u32>) {
    let mut text = String::new();
    let mut segs: Vec<u32> = Vec::new();
    for p in pieces {
        match p {
            Piece::Run(t) | Piece::Lit(_, t) => {
                if let Piece::Lit(i, _) = p
                    && let Some(c) = cmap.get_mut(*i)
                {
                    c.1 = t.clone();
                }
                // Text that goes on as the original's right after a copied stretch is
                // part of it (one segment, not two).
                let at = text.len() as u32;
                if let [.., ks, fs, len] = segs[..]
                    && ks + len == at
                {
                    let f = ftext.get((fs + len) as usize..).unwrap_or("").as_bytes();
                    let same = t.bytes().zip(f).take_while(|(x, y)| x == *y).count();
                    // (Whole characters.)
                    let same = (0..=same)
                        .rev()
                        .find(|&n| t.is_char_boundary(n))
                        .unwrap_or(0);
                    let n = segs.len();
                    segs[n - 1] += same as u32;
                }
                text.push_str(t);
            }
            Piece::Copy(a, b) => {
                let at = text.len() as u32;
                match segs[..] {
                    // (A copy that goes on the last one.)
                    [.., ks, fs, len] if ks + len == at && fs + len == *a => {
                        let n = segs.len();
                        segs[n - 1] += b - a;
                    }
                    _ => {
                        segs.push_out(at);
                        segs.push_out(*a);
                        segs.push_out(b - a);
                    }
                }
                text.push_str(ftext.get(*a as usize..*b as usize).unwrap_or(""));
            }
            Piece::Mark(i) => {
                if let Some(r) = scaf.get_mut(*i) {
                    r.2 = text.len() as u32;
                }
            }
        }
    }
    for &(a, b) in aliases {
        if let Some(v) = cmap.get(b).map(|c| c.1.clone())
            && let Some(c) = cmap.get_mut(a)
        {
            c.1 = v;
        }
    }
    (text, segs)
}

/// Writes activation `act`'s continuation and checks that its canonical activation makes
/// it again (D2913).
pub(super) fn generate(act: &Act, ctx: &mut dyn Ctx) -> R<KText> {
    let mut w = run(act, ctx, None)?;
    // (Replayed, the tape reads 0 past its end.)
    while w.tape.last() == Some(&0) {
        w.tape.pop();
    }
    let c = canonical(
        &act.site,
        act.syntax.clone(),
        act.ftext.clone(),
        act.fkind,
        act.kind,
        act.peers.clone(),
    );
    let again = |e: String| {
        cat!(
            "plain dumps: a continuation its origin does not make again (",
            e,
            ")"
        )
    };
    let r = run(&c, &mut Canon { act: &c }, Some(&w.tape)).map_err(again)?;
    let h = holes(&w, &r, &act.ftext).map_err(again)?;
    let (text, _) = layout(&w.pieces, &act.ftext, &mut w.scaf, &mut w.cmap, &w.aliases);
    let text = match &w.wrap {
        Some(p) => cat!("({ __proto__: ", p, ", ", text, " }).K"),
        None => text,
    };
    Ok(KText {
        text,
        used: w.used,
        args: w.args,
        holes: h,
        tape: w.tape,
        top: w.top,
    })
}

/// Where `w`'s pieces differ from `r`'s, which has the same shape: `[piece, length]` of
/// each run or literal whose text differs, unless it reads as `r`'s with a token (a
/// name, a reference, a literal) for each placeholder ([`scan`]).
fn holes(w: &Out, r: &Out, ftext: &str) -> R<Vec<u32>> {
    if w.pieces.len() != r.pieces.len() {
        return Err(cat!(w.pieces.len(), " pieces, ", r.pieces.len(), " again"));
    }
    let mut text = String::new();
    let mut starts = Vec::new();
    for p in &w.pieces {
        starts.push_out(text.len());
        match p {
            Piece::Run(t) | Piece::Lit(_, t) => text.push_str(t),
            Piece::Copy(a, b) => text.push_str(ftext.get(*a as usize..*b as usize).unwrap_or("")),
            Piece::Mark(_) => {}
        }
    }
    let mut h = Vec::new();
    let mut lits = Vec::new();
    for (i, (a, b)) in w.pieces.iter().zip(&r.pieces).enumerate() {
        let same = |at: usize, t: &str| text.as_bytes().get(at..at + t.len()) == Some(t.as_bytes());
        let mut differs = |x: &String, y: &String| {
            if x != y && scan(y, starts[i], &same, &text) != Some(x.len()) {
                h.push_out(i as u32);
                h.push_out(x.len() as u32);
            }
        };
        match (a, b) {
            (Piece::Run(x), Piece::Run(y)) => differs(x, y),
            (Piece::Lit(j, x), Piece::Lit(k, y)) if j == k => {
                lits.push_out(*j);
                differs(x, y)
            }
            (Piece::Copy(..), Piece::Copy(..)) | (Piece::Mark(_), Piece::Mark(_)) if a == b => {}
            _ => return Err(cat!("piece ", i, " differs")),
        }
    }
    let same_scaf = w.scaf.len() == r.scaf.len()
        && w.scaf
            .iter()
            .zip(&r.scaf)
            .all(|(x, y)| x.0 == y.0 && x.1 == y.1 && x.3 == y.3);
    if !same_scaf {
        return Err(String::from("its records differ"));
    }
    let same_cmap = w.cmap.len() == r.cmap.len()
        && w.cmap.iter().zip(&r.cmap).enumerate().all(|(i, (x, y))| {
            let filled = lits.contains(&i) || w.aliases.iter().any(|a| a.0 == i);
            x.0 == y.0 && x.2 == y.2 && (filled || x.1 == y.1)
        });
    if !same_cmap {
        return Err(String::from("its temporaries differ"));
    }
    Ok(h)
}

/// The length of the text at `at` of `k` that reads as `pattern` with a token for each
/// placeholder: fixed parts as `same` compares them.
fn scan(pattern: &str, at: usize, same: &dyn Fn(usize, &str) -> bool, k: &str) -> Option<usize> {
    let mut cur = at;
    for (n, part) in pattern.split(PH).enumerate() {
        if n > 0 {
            let t = token_len(k.get(cur..)?);
            if t == 0 {
                return None;
            }
            cur += t;
        }
        if !same(cur, part) {
            return None;
        }
        cur += part.len();
    }
    Some(cur - at)
}

/// The length of the value `t` starts with (D3005): a token, or an expression read up to
/// the first `,`, `;`, closing bracket or line end outside brackets, strings, templates,
/// comments and regular expressions, its spaces only before a name or a bracket
/// (`async function (…) {…}`, `new Map([…])`). The writer leaves out a hole only where
/// this gives the value's length in its own text, so the reader, scanning the same text,
/// finds the same.
fn token_len(t: &str) -> usize {
    let b = t.as_bytes();
    let mut i = 0;
    let mut depth = 0u32;
    // Template literals open: the bracket depth at each `${`.
    let mut tpl: Vec<u32> = Vec::new();
    // Whether a `/` here starts a regular expression.
    let mut regex_ok = true;
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' | b'\'' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != c {
                    if b[j] == b'\n' {
                        return 0;
                    }
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                if j >= b.len() {
                    return 0;
                }
                i = j + 1;
                regex_ok = false;
                continue;
            }
            b'`' => {
                i = match template_end(b, i + 1) {
                    Some((j, open)) => {
                        if open {
                            tpl.push_out(depth);
                            depth += 1;
                            regex_ok = true;
                        } else {
                            regex_ok = false;
                        }
                        j
                    }
                    None => return 0,
                };
                continue;
            }
            b'}' if depth > 0 && tpl.last() == Some(&(depth - 1)) => {
                // The end of a template's substitution: the template goes on.
                tpl.pop();
                depth -= 1;
                i = match template_end(b, i + 1) {
                    Some((j, open)) => {
                        if open {
                            tpl.push_out(depth);
                            depth += 1;
                            regex_ok = true;
                        } else {
                            regex_ok = false;
                        }
                        j
                    }
                    None => return 0,
                };
                continue;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                regex_ok = true;
            }
            b')' | b']' | b'}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                regex_ok = false;
            }
            b',' | b';' | b'\n' if depth == 0 => break,
            b' ' if depth == 0 => {
                // (After the spaces: a TypeScript original's blanked types are several.)
                let next = b[i..].iter().find(|&&c| c != b' ').copied().unwrap_or(b' ');
                if !(next.is_ascii_alphanumeric()
                    || matches!(next, b'_' | b'$' | b'(' | b'{' | b'['))
                {
                    break;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let Some(e) = t[i + 2..].find("*/") else {
                    return 0;
                };
                i += e + 4;
                continue;
            }
            b'/' if regex_ok => {
                let mut j = i + 1;
                let mut class = false;
                while j < b.len() && (class || b[j] != b'/') {
                    match b[j] {
                        b'\\' => j += 1,
                        b'[' => class = true,
                        b']' => class = false,
                        b'\n' => return 0,
                        _ => {}
                    }
                    j += 1;
                }
                if j >= b.len() {
                    return 0;
                }
                i = j + 1;
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
                regex_ok = false;
                continue;
            }
            _ => {}
        }
        if !c.is_ascii_whitespace() {
            // (After a name or a literal a `/` divides; after an operator it starts a
            // regular expression. A keyword before one is rare in a value: a miss only
            // keeps a hole.)
            regex_ok = !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$' | b')' | b']'));
        }
        i += 1;
    }
    if depth != 0 || !tpl.is_empty() {
        return 0;
    }
    // (No trailing spaces.)
    while i > 0 && b[i - 1] == b' ' {
        i -= 1;
    }
    i
}

/// Where the template literal text from `i` (after a backtick or a substitution's `}`)
/// ends: after its closing backtick (`false`) or after a `${` (`true`).
fn template_end(b: &[u8], mut i: usize) -> Option<(usize, bool)> {
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'`' => return Some((i + 1, false)),
            b'$' if b.get(i + 1) == Some(&b'{') => return Some((i + 2, true)),
            _ => i += 1,
        }
    }
    None
}

/// The layout of a continuation `ktext` of the original `act` stands for (canonical),
/// from its record's tape and holes: the writer's segments, scaffold records (kind,
/// offset in the original, offset in K, the rest as JSON) and temporaries.
pub(in super::super) struct Layout {
    pub segs: Vec<u32>,
    pub scaf: Vec<(&'static str, u32, u32, String)>,
    pub cmap: Vec<(String, String, bool)>,
}

///
/// `ts`: the original is TypeScript, which the dump wrote as JavaScript, its types blanked
/// (D2879): K's text has spaces where the original's has types.
pub(in super::super) fn layout_of(
    act: &Act,
    tape: &[u32],
    h: &[u32],
    ktext: &str,
    ts: bool,
) -> R<Layout> {
    let mut r = run(act, &mut Canon { act }, Some(tape))?;
    let starts = |at: usize, t: &str| {
        let k = ktext.as_bytes().get(at..at + t.len());
        k.is_some_and(|k| {
            k.iter()
                .zip(t.as_bytes())
                .all(|(&x, &y)| x == y || (ts && x == b' ' && y != b'\n'))
        })
    };
    let mut at = 0usize;
    let mut holes = h.chunks(2);
    let mut next = holes.next();
    let differs = || String::from("its text is not the one its origin makes");
    // The first byte each piece's successors expect (a TypeScript original's blanked
    // types, below).
    let mut after: Vec<Option<u8>> = alloc::vec![None; r.pieces.len()];
    let mut first = None;
    for (i, p) in r.pieces.iter().enumerate().rev() {
        after[i] = first;
        let f = match p {
            Piece::Run(t) | Piece::Lit(_, t) => t.as_bytes().first().copied(),
            Piece::Copy(a, b) if a < b => act.ftext.as_bytes().get(*a as usize).copied(),
            _ => None,
        };
        if f.is_some() {
            first = f;
        }
    }
    for (i, p) in r.pieces.iter_mut().enumerate() {
        let hole = match next {
            Some(&[j, len]) if j as usize == i => {
                next = holes.next();
                Some(len as usize)
            }
            _ => None,
        };
        // (Without a hole, a placeholder is the token there.)
        let hole = match p {
            Piece::Run(t) | Piece::Lit(_, t) if hole.is_none() && t.contains(PH) => {
                Some(scan(t, at, &starts, ktext).ok_or_else(differs)?)
            }
            _ => hole,
        };
        match p {
            Piece::Run(t) | Piece::Lit(_, t) => match hole {
                Some(len) => {
                    let s = ktext.get(at..at + len).ok_or_else(differs)?;
                    *t = String::from(s);
                    at += len;
                }
                None => {
                    if !starts(at, t) {
                        return Err(differs());
                    }
                    at += t.len();
                }
            },
            Piece::Copy(a, b) => {
                let t = act
                    .ftext
                    .get(*a as usize..*b as usize)
                    .ok_or_else(differs)?;
                if hole.is_some() || !starts(at, t) {
                    return Err(differs());
                }
                at += t.len();
                // The writer copied the TypeScript original, a node with its types
                // (`n1!`); the origin read back is the dump's JavaScript, the types blank,
                // where the node ends before them: the copy goes on over the blanks K has
                // where the next piece cannot start with one. (Whatever the origin's
                // script: a dump that blanked them is JavaScript.)
                {
                    let f = act.ftext.as_bytes();
                    let k = ktext.as_bytes();
                    while f.get(*b as usize) == Some(&b' ')
                        && k.get(at) == Some(&b' ')
                        && after[i].is_some_and(|c| c != b' ')
                    {
                        *b += 1;
                        at += 1;
                    }
                }
            }
            Piece::Mark(_) => {
                if hole.is_some() {
                    return Err(differs());
                }
            }
        }
    }
    if at != ktext.len() || next.is_some() {
        return Err(differs());
    }
    let (_, segs) = layout(&r.pieces, &act.ftext, &mut r.scaf, &mut r.cmap, &r.aliases);
    Ok(Layout {
        segs,
        scaf: r.scaf,
        cmap: r.cmap,
    })
}

/// The activation a continuation of `ftext` at `site` is written again from: the
/// site's slots, every value a placeholder.
pub(in super::super) fn canonical(
    site: &Site,
    syntax: Arc<Syntax>,
    ftext: String,
    fkind: u32,
    kind: i32,
    peers: Vec<String>,
) -> Act {
    let slots: Vec<Slot> = site
        .named_slots()
        .into_iter()
        .filter(|s| !super::super::flight::computed_hidden(&s.name))
        .map(|s| Slot {
            name: s.name,
            value: String::from(PH),
            cell: None,
        })
        .collect();
    let usings = slots
        .iter()
        .filter(|s| s.name.starts_with("using.record@"))
        .map(|s| Using {
            slot: s.name.clone(),
            triples: Vec::new(),
            state: Vec::new(),
        })
        .collect();
    let sup = syntax.special.iter().any(|&(k, _)| k == Special::Super);
    Act {
        name: String::new(),
        fexpr: String::new(),
        ftext,
        fkind,
        site: site.clone(),
        syntax,
        awaiting: None,
        this: String::from(PH),
        slots,
        queue: Vec::new(),
        kind,
        proto: None,
        peers,
        usings,
        rereads: Vec::new(),
        super_proto: sup.then(|| String::from(PH)),
    }
}

/// The dump as the canonical activation sees it: every value a placeholder, nothing made
/// later, no code of the dump's own.
struct Canon<'a> {
    act: &'a Act,
}

impl Ctx for Canon<'_> {
    fn slot(&mut self, name: &str) -> Option<String> {
        self.act
            .slots
            .iter()
            .any(|s| s.name == name)
            .then(|| String::from(PH))
    }
    fn cell(&mut self, _: &str) -> Option<Option<String>> {
        None
    }
    fn after_go(&self, _: &str) -> bool {
        false
    }
    fn rereads(&self, _: &str) -> bool {
        false
    }
    fn arg(&mut self, name: &str) -> Option<String> {
        self.slot(name)
    }
    fn made_in(&self, _: &str) -> Option<usize> {
        None
    }
    fn block(&mut self, _: usize, _: &[BDecl]) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    fn go(&self, _: usize) -> String {
        String::new()
    }
    fn awaiting(&mut self, _: &[String]) -> String {
        String::from(PH)
    }
    fn site_reads(&self) -> Vec<String> {
        Vec::new()
    }
}
