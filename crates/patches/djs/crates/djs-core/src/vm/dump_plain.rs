//! Plain dumps (format 3, D2830, docs/plain-js-dumps.md): the dump as a Script that runs
//! given only the host's functions, in djs and in node. Every suspended activation is a
//! *continuation* (dump_cont.rs): a function whose body is the rest of the original from
//! its site, which is also the factory of the scopes its frame shares with closures.
//!
//! The exporter writes the format 2 lines as always (its checker compiles them), with
//! the plain forms at the source (`$dump.bN` for builtins, `$djs.resume(...)` for open
//! calls, host functions by name), and records here what the lines do not say: the
//! scope groups' factories (variables, closures and what each captures) and the
//! activations (function, site, slots, cells). The writer then
//!
//! - *merges* the factories whose variables a frame shares: their variables become
//!   declarations of continuations, their closures are made inside them (placement);
//! - *schedules* the lines: a continuation is called where its activation was, unless
//!   code written before needs what its prefix makes; then it is called there and what
//!   lies between goes into a callback its prefix calls right before the site;
//! - writes a value used once where it is used (D3001, dump_plain_inline.rs), the others
//!   on a plain global `$dump` (which the host deletes after the load, D2916), and the
//!   run's handle as the Script's completion value.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::compiler::Site;

#[path = "dump_plain_inline.rs"]
mod inline;

/// What the plain writer needs besides the format 2 lines.
#[derive(Default)]
pub(super) struct Ir {
    /// Builtins no expression on the globals reaches, by path: `$dump.b{i}`.
    pub builtins: Vec<String>,
    pub factories: Vec<Factory>,
    pub acts: Vec<Act>,
    /// The line of `out` that writes a factory or an activation.
    pub lines: Vec<(usize, Rec)>,
    /// `Math.random`'s state, where the run called it (D2837).
    pub seed: Option<(i32, i32)>,
    /// The id the VM's next host call gets, where it is not one past the highest
    /// reopened call's: `$djs.seed`'s third argument, or its only one (D2897, D2837).
    pub next_call: Option<u32>,
    /// The handles, in the host's order: name, expression.
    pub handles: Vec<(String, String)>,
}

#[derive(Clone, Copy)]
pub(super) enum Rec {
    Factory(usize),
    Act(usize),
}

/// A scope group's factory (`$dump.sN = (() => {...})()`), by scopes: scope 0 is the
/// factory, the others nested factories whose objects their parent spreads.
pub(super) struct Factory {
    /// `sN`.
    pub name: String,
    pub scopes: Vec<Scope>,
}

pub(super) struct Scope {
    /// The enclosing scope (`usize::MAX` for scope 0).
    pub parent: usize,
    /// Statements that are not variables or closures (class definitions).
    pub other: Vec<String>,
    pub vars: Vec<Var>,
    pub props: Vec<Prop>,
}

pub(super) struct Var {
    pub name: String,
    /// Its line in the factory (empty in its TDZ).
    pub line: String,
    /// `None`: declared without a value (set later by `$set`).
    pub init: Option<String>,
    /// In its TDZ (declared after the factory's `return`).
    pub tdz: bool,
    /// Its box's node.
    pub boxn: usize,
    /// A class statement's binding (its line is the class).
    pub class: bool,
}

/// A member of a scope's returned object.
pub(super) struct Prop {
    pub key: String,
    /// As written in the object: `key`, `key: expr`, a method.
    pub text: String,
    /// The boxes (variables) it captures.
    pub caps: Vec<usize>,
}

pub(super) struct Act {
    /// `aN`.
    pub name: String,
    /// The expression the dump reaches the function by.
    pub fexpr: String,
    /// The function's text and its kind (`func_flags::KIND_*`).
    pub ftext: String,
    pub fkind: u32,
    pub site: Site,
    pub syntax: alloc::sync::Arc<crate::compiler::Syntax>,
    pub awaiting: Option<String>,
    pub this: String,
    pub slots: Vec<Slot>,
    /// An async generator's requests: type, value, promise.
    pub queue: Vec<(&'static str, String, String)>,
    /// `suspend::ASYNC`, `GENERATOR`, `ASYNC_GENERATOR`.
    pub kind: i32,
    /// A generator object's prototype (its continuation's `prototype`).
    pub proto: Option<String>,
    /// The names of the sites of its site's kind at its site's offset, in order (a `for
    /// await`'s closes, an async `yield*`'s awaits of inner results; empty if unknown).
    pub peers: Vec<String>,
    /// The `using` records of its frame.
    pub usings: Vec<Using>,
    /// Its temporaries the continuation evaluates again from the original's text
    /// (D2836, dump_reread.rs).
    pub rereads: Vec<String>,
    /// A method that uses `super`: its home object's prototype (its continuation is a
    /// method of an object of that prototype).
    pub super_proto: Option<String>,
}

/// The `using` resources of a scope in flight (D2876): `[value, method, kind]` and the
/// disposal's state, by field name (`hasAwaited`, `next`, `value`, …).
#[derive(Clone)]
pub(super) struct Using {
    /// The record's slot (`using.record@39`); while the exporter writes, its text.
    pub slot: String,
    pub triples: Vec<(String, String, i32)>,
    pub state: Vec<(String, String)>,
}

pub(super) struct Slot {
    pub name: String,
    pub value: String,
    /// A variable the frame shares with closures: its box's node.
    pub cell: Option<usize>,
}

// ---------------------------------------------------------------- references

/// What a piece of the dump's text refers to: `$dump.X` (a location), `$dump.X.k` (a
/// key of a factory's object, `key` set) and bare names (the dump's top-level
/// declarations among them).
#[derive(Default)]
pub(super) struct Refs {
    /// `(name, key)`: `$dump.name` or `$dump.name.key`.
    pub dump: Vec<(String, Option<String>)>,
    pub names: Vec<String>,
}

fn ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80
}

fn ident_part(c: u8) -> bool {
    ident_start(c) || c.is_ascii_digit()
}

/// The references of `text`, JavaScript the exporter wrote (data, factories, function
/// sources): strings, template text, comments and regular expressions are skipped; a
/// name after `.` is a property, not a reference. It errs on the side of too many
/// names (an object literal's keys count), which only adds orderings that hold anyway.
pub(super) fn refs(text: &str) -> Refs {
    scan(text, false, &|_| true, &mut |_, _| {}, true, false)
}

/// Where `text` reads `$dump.X` (as [`refs`] finds them): `[start, end)` of each, the
/// name's end (not a key after it).
pub(super) fn scan_spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if !has_dollar(text, "$dump.") {
        return out;
    }
    scan(
        text,
        false,
        &|_| false,
        &mut |s, e| out.push_out((s, e)),
        false,
        false,
    );
    out
}

/// [`scan_spans`] outside function and class bodies: the references a text's own
/// evaluation makes (a continuation's text inside it has its layout already, D3003).
pub(super) fn eager_spans(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if !has_dollar(text, "$dump.") {
        return out;
    }
    scan(
        text,
        true,
        &|_| false,
        &mut |s, e| out.push_out((s, e)),
        false,
        false,
    );
    out
}

/// The names `text`'s own evaluation reads: outside function and class bodies (read when
/// they run), object literals' keys left out.
pub(super) fn eager_names(text: &str) -> Vec<String> {
    scan(text, true, &|_| true, &mut |_, _| {}, false, true).names
}

/// The references of `text` its evaluation makes: [`refs`] without those inside function
/// bodies and class bodies (read when they run). A text of the dump, a value: a
/// `{` after `)` or `=>` opens a function's body (no statements are scanned).
#[cfg(test)]
fn eager_refs(text: &str) -> Refs {
    scan(text, true, &|_| true, &mut |_, _| {}, true, false)
}

/// [`refs`]'s or [`eager_refs`]'s with only the names `keep` takes (a data literal's
/// keys are names: most texts' names are wanted by no one, 0215).
fn scan(
    text: &str,
    skip_fns: bool,
    keep: &dyn Fn(&str) -> bool,
    span: &mut dyn FnMut(usize, usize),
    // (Whether `Refs::dump` is wanted: the span callers want only the spans.)
    collect: bool,
    // (Whether object literals' keys are left out of the names.)
    skip_keys: bool,
) -> Refs {
    let b = text.as_bytes();
    let mut out = Refs::default();
    // What `out` holds, for deduplication: a dump's data names thousands (0215: the
    // linear searches were most of writing a data-heavy dump).
    // (By hash, checked against `out`: a hash two of them share searches it, 0215.)
    let mut seen_dump = super::Index::default();
    let mut seen_names = super::Index::default();
    let mut i = 0;
    // Brackets of every kind open, and with `skip_fns` the function body being skipped:
    // its nesting, and whether it is an arrow's expression body.
    let mut nest: u32 = 0;
    let mut lazy: Option<(u32, bool)> = None;
    let mut arrow = false;
    let mut prev_close = false;
    let mut class = false;
    let mut after_class = false;
    // Template literal nesting: brace depth at each `${`.
    let mut tpl: Vec<u32> = Vec::new();
    let mut depth: u32 = 0;
    // Whether a `/` here starts a regular expression (after an operator or `(`).
    let mut regex_ok = true;
    let mut after_dot = false;
    while i < b.len() {
        let c = b[i];
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
            continue;
        }
        // (Most of a data literal: what the general steps below do for these, 0215. An
        // arrow's concise body a `,` would start it ends at once.)
        if c == b',' || c == b':' && !arrow {
            if let Some((n, true)) = lazy
                && nest == n
                && c == b','
            {
                lazy = None;
            }
            arrow = false;
            prev_close = false;
            class = class && c == b':';
            after_dot = false;
            regex_ok = true;
            i += 1;
            continue;
        }
        if skip_fns && lazy.is_none() {
            if c == b'{' && (arrow || prev_close || class) {
                lazy = Some((nest, false));
            } else if arrow {
                lazy = Some((nest, true));
            }
        }
        if let Some((n, true)) = lazy
            && nest == n
            && matches!(c, b',' | b';')
        {
            lazy = None;
        }
        arrow = false;
        prev_close = c == b')';
        if c != b'{' {
            class = class && !matches!(c, b';' | b',' | b')' | b']' | b'}');
        }
        match c {
            b'(' | b'[' => nest += 1,
            b')' | b']' => {
                nest = nest.saturating_sub(1);
                if let Some((n, _)) = lazy
                    && nest < n
                {
                    lazy = None;
                }
            }
            b'=' if b.get(i + 1) == Some(&b'>') => {
                i += 2;
                arrow = true;
                regex_ok = true;
                after_dot = false;
                continue;
            }
            _ => {}
        }
        match c {
            b'"' | b'\'' => {
                i = string_end(text, i + 1, c as char);
                regex_ok = false;
                after_dot = false;
                continue;
            }
            b'`' => {
                i = template_text(b, i + 1, &mut tpl, depth);
                // (After `${` an expression starts: `${/a/g.source}`.)
                regex_ok = b[..i].ends_with(b"${");
                after_dot = false;
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            b'/' if regex_ok => {
                i += 1;
                let mut class = false;
                while i < b.len() && (b[i] != b'/' || class) && b[i] != b'\n' {
                    match b[i] {
                        b'\\' => i += 1,
                        b'[' => class = true,
                        b']' => class = false,
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
                while i < b.len() && ident_part(b[i]) {
                    i += 1;
                }
                regex_ok = false;
                after_dot = false;
                continue;
            }
            b'{' => {
                depth += 1;
                nest += 1;
                class = false;
            }
            b'}' => {
                if tpl.last() == Some(&depth) {
                    tpl.pop();
                    i = template_text(b, i + 1, &mut tpl, depth);
                    regex_ok = b[..i].ends_with(b"${");
                    after_dot = false;
                    continue;
                }
                depth = depth.saturating_sub(1);
                nest = nest.saturating_sub(1);
                if let Some((n, concise)) = lazy
                    && (nest < n || nest == n && !concise)
                {
                    lazy = None;
                    // A function's body ends an expression (`() => {}` is no call).
                    prev_close = false;
                }
            }
            _ => {}
        }
        if ident_start(c) {
            let s = i;
            while i < b.len() && ident_part(b[i]) {
                i += 1;
            }
            let word = &text[s..i];
            // A class's own name (`class A`) binds, it reads nothing.
            let class_name = core::mem::take(&mut after_class) && word != "extends";
            if c == b'c' && !after_dot && word == "class" && b.get(i) != Some(&b':') {
                after_class = true;
                class = true;
            }
            if !after_dot && lazy.is_none() && !class_name {
                if c == b'$' && word == "$dump" && b.get(i) == Some(&b'.') {
                    let (name, j) = ident_at(text, i + 1);
                    if !name.is_empty() {
                        span(s, j);
                        let (key, k) = if b.get(j) == Some(&b'.') {
                            ident_at(text, j + 1)
                        } else {
                            ("", j)
                        };
                        let same = |d: &(String, Option<String>)| {
                            d.0 == name && d.1.as_deref().unwrap_or("") == key
                        };
                        if collect
                            && first_seen(
                                &mut seen_dump,
                                inline::name_hash(name).rotate_left(17) ^ inline::name_hash(key),
                                out.dump.len(),
                                &out.dump,
                                same,
                            )
                        {
                            let key = (!key.is_empty()).then(|| String::from(key));
                            out.dump.push_out((String::from(name), key));
                        }
                        i = k;
                        regex_ok = false;
                        after_dot = false;
                        continue;
                    }
                } else if !(skip_keys && key_at(b, s, i))
                    && keep(word)
                    && first_seen(
                        &mut seen_names,
                        inline::name_hash(word),
                        out.names.len(),
                        &out.names,
                        |n: &String| n == word,
                    )
                {
                    out.names.push_out(String::from(word));
                }
            }
            // After a keyword an expression starts (`return /x/`).
            regex_ok = word.len() <= 10
                && matches!(
                    c,
                    b'r' | b't' | b'i' | b'o' | b'n' | b'd' | b'v' | b'c' | b'e' | b'y' | b'a'
                )
                && matches!(
                    word,
                    "return"
                        | "typeof"
                        | "instanceof"
                        | "in"
                        | "of"
                        | "new"
                        | "delete"
                        | "void"
                        | "throw"
                        | "case"
                        | "do"
                        | "else"
                        | "yield"
                        | "await"
                );
            after_dot = false;
            continue;
        }
        if c.is_ascii_digit() {
            while i < b.len() && (ident_part(b[i]) || b[i] == b'.') {
                i += 1;
            }
            regex_ok = false;
            after_dot = false;
            continue;
        }
        after_dot = c == b'.' && !(b.get(i + 1) == Some(&b'.'));
        if c == b'.' && b.get(i + 1) == Some(&b'.') {
            // `...`
            i += 3;
            regex_ok = true;
            continue;
        }
        regex_ok = !matches!(c, b')' | b']' | b'}');
        i += 1;
    }
    out
}

/// Whether the entry of hash `h` that would go at `next` of `list` is not in `list` yet:
/// `seen` holds a hash's first entry; where another one shares it, `list` is searched.
fn first_seen<T>(
    seen: &mut super::Index,
    h: u64,
    next: usize,
    list: &[T],
    same: impl Fn(&T) -> bool,
) -> bool {
    let h = h.max(1);
    match seen.get(h) {
        None => {
            seen.insert(h, next as u32);
            true
        }
        Some(e) if same(&list[e as usize]) => false,
        Some(_) => !list.iter().any(same),
    }
}

/// Whether the word `b[s..e]` is an object literal's key (`{ key: …`, `, key: …`).
fn key_at(b: &[u8], s: usize, e: usize) -> bool {
    let next = b[e..]
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .map(|k| e + k);
    next.is_some_and(|k| b[k] == b':' && b.get(k + 1) != Some(&b':'))
        && b[..s]
            .iter()
            .rev()
            .find(|c| !c.is_ascii_whitespace())
            .is_some_and(|&c| c == b'{' || c == b',')
}

/// Whether `text` reads a name `allowed` does not hold (object literals' keys are no
/// reads), 0215: by `scan` without collecting names.
pub(super) fn reads_other(text: &str, allowed: &[&str]) -> bool {
    !scan(
        text,
        false,
        &|w| !allowed.contains(&w),
        &mut |_, _| {},
        false,
        true,
    )
    .names
    .is_empty()
}

/// Whether item text `t` names what makes a value with effects: `Promise.all/any/race`,
/// `.then`, `.next()`, `$djs.resume`, `$djs.k(`, a continuation `$dump.K…`. (One pass by
/// the dots, 0215: a data line is long.)
/// `text.split_once(" = ")`, at once for a line `$dump.X = …` (0215: a substring search
/// sets up a searcher per text).
pub(super) fn split_def(text: &str) -> Option<(&str, &str)> {
    if let Some(rest) = text.strip_prefix("$dump.") {
        let (name, at) = ident_at(rest, 0);
        if !name.is_empty() && rest[at..].starts_with(" = ") {
            let e = "$dump.".len() + at;
            return Some((&text[..e], &text[e + 3..]));
        }
    }
    text.split_once(" = ")
}

/// `text.match_indices(pat)`'s offsets for a `pat` that starts with `$`: by the `$`s,
/// which `find(char)` searches fast (0215: a substring search sets up a searcher per
/// text, most of the cost for the many short items of a data dump). (No match of `pat`
/// overlaps another: none has a `$` after its first byte.)
pub(super) fn dollar_indices<'t>(text: &'t str, pat: &'t str) -> impl Iterator<Item = usize> + 't {
    let mut at = 0;
    let b = text.as_bytes();
    core::iter::from_fn(move || {
        while let Some(i) = find_byte(b, at, b'$') {
            if b[i..].starts_with(pat.as_bytes()) {
                at = i + pat.len();
                return Some(i);
            }
            at = i + 1;
        }
        at = text.len() + 1;
        None
    })
}

/// `text.contains(pat)` for a `pat` that starts with `$` ([`dollar_indices`]).
pub(super) fn has_dollar(text: &str, pat: &str) -> bool {
    dollar_indices(text, pat).next().is_some()
}

fn find_dollar(text: &str, pat: &str) -> Option<usize> {
    dollar_indices(text, pat).next()
}

fn effect_marker(t: &str) -> bool {
    let b = t.as_bytes();
    let mut from = 0;
    let mut dots = core::iter::from_fn(|| {
        let at = find_byte(b, from, b'.')?;
        from = at + 1;
        Some(at)
    });
    dots.any(|at| {
        let before = &t[..at];
        let after = &b[at + 1..];
        let ends = |p: &str| before.ends_with(p);
        let starts = |p: &[u8]| after.starts_with(p);
        (ends("Promise") && (starts(b"all") || starts(b"any") || starts(b"race")))
            || starts(b"then")
            || starts(b"next()")
            || (ends("$djs") && (starts(b"resume") || starts(b"k(")))
            || (ends("$dump") && starts(b"K"))
    })
}

/// Where the string literal whose text starts at `i` ends (after its closing `q`): by
/// [`find_byte`], not byte by byte (0215: a data-heavy dump is mostly strings).
fn string_end(text: &str, i: usize, q: char) -> usize {
    let b = text.as_bytes();
    let mut from = i;
    loop {
        // (`q` is ASCII: its byte is the char.)
        let Some(at) = find_byte(b, from, q as u8) else {
            return b.len() + 1;
        };
        // Escaped where an odd number of backslashes (of the string's) stands before it.
        let slashes = b[i..at].iter().rev().take_while(|&&c| c == b'\\').count();
        if slashes % 2 == 0 {
            return at + 1;
        }
        from = at + 1;
    }
}

/// The first byte `x` of `b` at or after `from`: eight bytes at a time (0215: a data
/// dump is mostly strings, and a searcher set up per string cost more than the search).
pub(super) fn find_byte(b: &[u8], from: usize, x: u8) -> Option<usize> {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let pat = ONES * u64::from(x);
    let mut i = from;
    while i + 8 <= b.len() {
        let mut w = [0u8; 8];
        w.copy_from_slice(&b[i..i + 8]);
        let v = u64::from_le_bytes(w) ^ pat;
        // (A zero byte of `v` is an `x`; the lowest flag is the first one exactly.)
        let z = v.wrapping_sub(ONES) & !v & HIGH;
        if z != 0 {
            return Some(i + (z.trailing_zeros() / 8) as usize);
        }
        i += 8;
    }
    b.get(i..)?.iter().position(|&c| c == x).map(|k| i + k)
}

/// Skips a template literal's text from `i` to its end or its next `${`.
fn template_text(b: &[u8], mut i: usize, tpl: &mut Vec<u32>, depth: u32) -> usize {
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'`' => return i + 1,
            b'$' if b.get(i + 1) == Some(&b'{') => {
                tpl.push(depth);
                return i + 2;
            }
            _ => i += 1,
        }
    }
    i
}

fn ident_at(text: &str, i: usize) -> (&str, usize) {
    let b = text.as_bytes();
    let mut j = i;
    while j < b.len() && ident_part(b[j]) {
        j += 1;
    }
    if i < b.len() && ident_start(b[i]) {
        (&text[i..j], j)
    } else {
        ("", i)
    }
}

// ---------------------------------------------------------------- the writer

/// A scope of a factory: (factory, scope).
type Sc = (usize, usize);

/// Where code is written: the dump's top level, block `i` of activation `a`'s
/// continuation, the block of a merged scope's variables no frame takes (a dead group),
/// or an unmerged factory.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pl {
    Top,
    Block(usize, usize),
    Group(Sc),
    Fac(Sc),
}

/// What is written at a place.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Put {
    Prop(Sc, usize),
    /// A merged scope's class statement (`scope.other[i]`): where the members that
    /// name it are.
    Other(Sc, usize),
    Sub(Sc),
    Group(Sc),
    K(usize),
}

#[derive(Clone, PartialEq, Eq)]
enum IK {
    /// `let`/`const`/`var`/`class` at the top: the names it declares.
    Decl(Vec<String>),
    /// A function declaration (hoisted to the head).
    FnDecl,
    /// `$dump.name = ...`.
    Def(String),
    Stmt,
    Factory(usize),
    Act(usize),
}

struct Item {
    kind: IK,
    text: String,
    deps: Vec<usize>,
    /// An activation's: what its awaited promise needs; per slot, what its value needs.
    start_deps: Vec<usize>,
    slot_deps: Vec<(String, Vec<usize>)>,
    /// An activation's: what defines its continuation (never moved into its callback).
    def_deps: Vec<usize>,
    pure: bool,
}

/// A binding a block of a continuation declares (the generator's dry run lists them).
#[derive(Clone)]
pub(super) struct BDecl {
    pub name: String,
    /// Its slot name in the frame, if it has one.
    pub sname: Option<String>,
}

/// What the generator asks of the dump while it writes a continuation.
pub(super) trait Ctx {
    /// The expression of slot `name`, if the frame has it.
    fn slot(&mut self, name: &str) -> Option<String>;
    /// A variable the frame shares with closures: `Some(None)` in its TDZ, `Some(value)`
    /// else; `None`: not shared.
    fn cell(&mut self, sname: &str) -> Option<Option<String>>;
    /// The binding's value exists only after the dump's callback ran.
    fn after_go(&self, sname: &str) -> bool;
    /// The temporary is evaluated again from the original's text (D2836).
    fn rereads(&self, slot: &str) -> bool;
    /// The value of slot `name` as an argument of the continuation's call (D2909).
    fn arg(&mut self, name: &str) -> Option<String>;
    /// The part of the dump's callback that makes what `text` reads, if it does (D2907).
    fn made_in(&self, text: &str) -> Option<usize>;
    /// The code the dump puts in the continuation's block `index`, by statement.
    fn block(&mut self, index: usize, info: &[BDecl]) -> Result<Vec<String>, String>;
    /// The call of the part of the dump's callback that runs once block `block` of the
    /// continuation has its code, or "" (D2907).
    fn go(&self, block: usize) -> String;
    /// `scope`: the names K may bind where the operand stands.
    fn awaiting(&mut self, scope: &[String]) -> String;
    /// The bindings of K the dump named in the site's operand (D3003).
    fn site_reads(&self) -> Vec<String>;
}

/// A continuation's text and its origin record (D2833).
#[derive(Clone, Default)]
pub(super) struct KText {
    pub text: String,
    /// The slots it consumed.
    pub used: Vec<String>,
    /// Its call's arguments, for its parameters (D2909).
    pub args: Vec<String>,
    /// Its record's holes and tape (D2913).
    pub holes: Vec<u32>,
    pub tape: Vec<u32>,
    /// The names it binds in its own scope (its parameters and body's declarations):
    /// what its text reads of them is its own (D3001).
    pub top: Vec<String>,
}

#[derive(Default)]
struct AState {
    /// Where its function lives: a prop of a factory scope.
    home: Option<(Sc, usize)>,
    /// Its slots that are variables of merged factories: slot name, (scope, var).
    cells: Vec<(String, Sc, usize)>,
    /// Its blocks' declarations, from the dry run.
    blocks: Vec<Vec<BDecl>>,
    place: Option<Pl>,
    /// Slot values written before the continuation (they read names it may shadow).
    pre: Vec<(String, String)>,
    after_go: Vec<String>,
    needs_go: bool,
    /// The calls of its callback's parts, by the block of the continuation each runs at
    /// the end of (D2907).
    go: Vec<String>,
    /// What its callback makes (`$dump.` names), by the part that makes it (D2907): a
    /// binding or a loop of K reads it only once that part has run.
    made: Vec<(String, usize)>,
    k: Option<KText>,
    blocks_done: bool,
    /// The site's operand, taken from the last part of its callback (D3002), and the
    /// bindings of K it names (D3003).
    site_val: Option<String>,
    site_reads: Vec<String>,
}

/// One entry of the schedule: an item, or an activation's call with its callback.
#[derive(Clone)]
enum Plan {
    Item(usize),
    Call(usize, Option<Vec<Plan>>),
}

/// The activation and slot name that own a factory variable.
type Owner = (usize, String);

struct W<'x> {
    ir: &'x Ir,
    items: Vec<Item>,
    acts: Vec<AState>,
    /// Per scope: whether a frame takes one of its variables.
    merged: Vec<Vec<bool>>,
    /// Per (factory, scope, var): the activation and slot that own it, and its block.
    owner: Vec<Vec<Vec<Option<Owner>>>>,
    vblock: Vec<Vec<Vec<Option<Pl>>>>,
    /// Box node -> (scope, var).
    boxes: Vec<(usize, Sc, usize)>,
    placed: Vec<(Pl, Put)>,
    act_place: Vec<Option<Pl>>,
    group_place: Vec<(Sc, Pl)>,
    /// Class statements of merged scopes written as expressions in a continuation's
    /// block (its own binding is the frame's there): scope, class name, local.
    class_exprs: Vec<(Sc, String, String)>,
    in_progress: Vec<Sc>,
    kkeys: Vec<String>,
    /// The dump's top-level names (roots, functions, classes), and as a set.
    globals: Vec<String>,
    global_set: alloc::collections::BTreeSet<String>,
    /// The globals' first bytes (a bit each) and longest length: a filter before the set.
    global_first: [u64; 4],
    global_max: usize,
    /// The hashes of `global_set`'s names: most words a filter above lets through are no
    /// global (0215).
    global_hashes: inline::Counts,
    /// Factories by name, and the item of each activation (0215: linear searches made
    /// writing a data-heavy dump quadratic).
    fac_by_name: alloc::collections::BTreeMap<String, usize>,
    act_items: Vec<usize>,
    write_k: &'x dyn Fn(&Act, &mut dyn Ctx) -> Result<KText, String>,
    /// Single-use values written in place (D3001).
    inl: inline::Inline,
    /// The activation whose line is being written (its continuation's site may take an
    /// effectful value), and whether its prefix does something before the site.
    kline: Option<usize>,
    k_effects: bool,
    /// Slot values of continuations with values written in place, by activation and slot.
    vcache: alloc::collections::BTreeMap<(usize, String), String>,
    /// [`W::dep_items`]'s marks: the call that last listed each item.
    dep_mark: Vec<u32>,
    dep_stamp: u32,
    /// Members of merged scopes written where they are used instead (D3003).
    taken: Vec<(Sc, usize)>,
    /// Continuations of merged scopes written where they are called instead (D3003).
    taken_k: Vec<usize>,
    /// Per activation, the names its continuation may bind ([`W::k_scope`]).
    kscopes: Vec<alloc::sync::Arc<alloc::collections::BTreeSet<String>>>,
}

type R<T> = Result<T, String>;

/// Whether `t` is a function expression (its body runs only when called).
fn is_fn_text(t: &str) -> bool {
    let t = t.trim_start();
    // `(0, f)`: an anonymous function kept anonymous (D2902).
    if let Some(inner) = t.strip_prefix("(0, ").and_then(|t| t.strip_suffix(')'))
        && is_fn_text(inner)
    {
        return true;
    }
    let t = t.strip_prefix("async ").map_or(t, str::trim_start);
    if t.starts_with("function") {
        return true;
    }
    let b = t.as_bytes();
    let rest = if b.first() == Some(&b'(') {
        let mut depth = 0u32;
        let mut end = None;
        for (i, &c) in b.iter().enumerate() {
            match c {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i + 1);
                        break;
                    }
                }
                b'"' | b'\'' | b'`' => return false,
                _ => {}
            }
        }
        match end {
            Some(e) => &t[e..],
            None => return false,
        }
    } else {
        let n = b
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_' || **c == b'$')
            .count();
        if n == 0 {
            return false;
        }
        &t[n..]
    };
    rest.trim_start().starts_with("=>")
}

fn refuse<T>(why: &str) -> R<T> {
    Err(String::from(why))
}

/// Writes the plain dump: `lines` are the format 2 lines (`out`, then the tail of
/// builtin changes, patches and finals); `write_k` writes a continuation.
pub(super) fn write(
    ir: &Ir,
    lines: &[String],
    ts: bool,
    write_k: &dyn Fn(&Act, &mut dyn Ctx) -> Result<KText, String>,
) -> R<String> {
    let mut w = W {
        ir,
        items: Vec::new(),
        acts: (0..ir.acts.len()).map(|_| AState::default()).collect(),
        merged: ir
            .factories
            .iter()
            .map(|f| alloc::vec![false; f.scopes.len()])
            .collect(),
        owner: ir
            .factories
            .iter()
            .map(|f| {
                f.scopes
                    .iter()
                    .map(|s| alloc::vec![None; s.vars.len()])
                    .collect()
            })
            .collect(),
        vblock: ir
            .factories
            .iter()
            .map(|f| {
                f.scopes
                    .iter()
                    .map(|s| alloc::vec![None; s.vars.len()])
                    .collect()
            })
            .collect(),
        boxes: Vec::new(),
        placed: Vec::new(),
        act_place: alloc::vec![None; ir.acts.len()],
        group_place: Vec::new(),
        class_exprs: Vec::new(),
        in_progress: Vec::new(),
        globals: Vec::new(),
        global_set: Default::default(),
        global_first: [0; 4],
        global_max: 0,
        global_hashes: Default::default(),
        fac_by_name: ir
            .factories
            .iter()
            .enumerate()
            .rev()
            .map(|(i, f)| (f.name.clone(), i))
            .collect(),
        act_items: Vec::new(),
        // A continuation's key in its factory: `$K<act>`, unless a member has that name
        // already (a continuation of an earlier dump, named by its key there).
        kkeys: ir
            .acts
            .iter()
            .map(|a| {
                let mut k = cat!("$K", a.name);
                while ir
                    .factories
                    .iter()
                    .any(|f| f.scopes.iter().any(|s| s.props.iter().any(|p| p.key == k)))
                {
                    k.push('_');
                }
                k
            })
            .collect(),
        write_k,
        inl: Default::default(),
        kline: None,
        k_effects: false,
        vcache: Default::default(),
        dep_mark: Vec::new(),
        dep_stamp: 0,
        taken: Vec::new(),
        taken_k: Vec::new(),
        kscopes: ir
            .acts
            .iter()
            .map(|act| {
                let mut scope = alloc::collections::BTreeSet::new();
                for nd in &act.syntax.nodes {
                    scope.extend(nd.decls.iter().map(|d| d.name.clone()));
                }
                for sl in &act.slots {
                    scope.insert(String::from(sl.name.split('@').next().unwrap_or(&sl.name)));
                }
                alloc::sync::Arc::new(scope)
            })
            .collect(),
    };
    for (f, fac) in ir.factories.iter().enumerate() {
        for (s, sc) in fac.scopes.iter().enumerate() {
            for (v, var) in sc.vars.iter().enumerate() {
                w.boxes.push_out((var.boxn, (f, s), v));
            }
        }
    }
    w.read_items(lines)?;
    for n in &w.global_set {
        if let Some(&c) = n.as_bytes().first() {
            w.global_first[usize::from(c >> 6)] |= 1u64 << (c & 63);
            w.global_max = w.global_max.max(n.len());
            w.global_hashes.add(inline::name_hash(n));
        }
    }
    w.activations()?;
    w.place_all()?;
    let plan = w.schedule()?;
    w.count_uses();
    w.emit(&plan, ts)
}

impl W<'_> {
    fn scope(&self, sc: Sc) -> &Scope {
        &self.ir.factories[sc.0].scopes[sc.1]
    }

    fn parent_scope(&self, sc: Sc) -> Option<Sc> {
        let p = self.scope(sc).parent;
        (p != usize::MAX).then_some((sc.0, p))
    }

    fn box_var(&self, b: usize) -> Option<(Sc, usize)> {
        self.boxes
            .iter()
            .find(|x| x.0 == b)
            .map(|&(_, sc, v)| (sc, v))
    }

    fn is_merged(&self, sc: Sc) -> bool {
        self.merged[sc.0][sc.1]
    }

    // ------------------------------------------------------------ items

    fn read_items(&mut self, lines: &[String]) -> R<()> {
        let mut recs: alloc::collections::BTreeMap<usize, Rec> = Default::default();
        for x in self.ir.lines.iter().rev() {
            recs.insert(x.0, x.1);
        }
        for (i, l) in lines.iter().enumerate() {
            let rec = recs.get(&i).copied();
            let kind = match rec {
                Some(Rec::Factory(f)) => IK::Factory(f),
                Some(Rec::Act(a)) => IK::Act(a),
                None => {
                    if l.starts_with("$dump.handle(") {
                        continue;
                    }
                    classify(l)
                }
            };
            match &kind {
                IK::Decl(names) => {
                    self.globals.extend(names.iter().cloned());
                    self.global_set.extend(names.iter().cloned());
                }
                IK::FnDecl => {
                    if let Some(n) = declared_fn_name(l) {
                        self.global_set.insert(n.clone());
                        self.globals.push_out(n);
                    }
                }
                IK::Act(a) => {
                    if self.act_items.len() <= *a {
                        self.act_items.resize(*a + 1, usize::MAX);
                    }
                    if self.act_items[*a] == usize::MAX {
                        self.act_items[*a] = self.items.len();
                    }
                }
                _ => {}
            }
            self.items.push(Item {
                kind,
                text: l.clone(),
                deps: Vec::new(),
                start_deps: Vec::new(),
                slot_deps: Vec::new(),
                def_deps: Vec::new(),
                pure: false,
            });
        }
        Ok(())
    }

    // ------------------------------------------------------------ activations

    /// The function's home and cells of every activation, the merged scopes, and the
    /// blocks of each continuation (a dry run of the generator).
    fn activations(&mut self) -> R<()> {
        let ir = self.ir;
        for (a, act) in ir.acts.iter().enumerate() {
            // Its function: a closure of a factory (`$dump.sN.key`), or written at the top.
            if let Some(rest) = act.fexpr.strip_prefix("$dump.")
                && let Some((top, key)) = rest.split_once('.')
                && let Some(f) = ir.factories.iter().position(|f| f.name == top)
            {
                // (A method of a class the factory makes, `Tagged.prototype.tag`: its
                // continuation lives where the class does, seeing the same variables.)
                let find = |key: &str| {
                    ir.factories[f]
                        .scopes
                        .iter()
                        .enumerate()
                        .find_map(|(s, sc)| {
                            sc.props
                                .iter()
                                .position(|p| p.key == key)
                                .map(|p| ((f, s), p))
                        })
                };
                let found = find(key).or_else(|| {
                    key.split_once('.')
                        .filter(|(_, rest)| rest.starts_with("prototype.") || !rest.contains('.'))
                        .and_then(|(class, _)| find(class))
                });
                match found {
                    Some(h) => self.acts[a].home = Some(h),
                    None => return refuse("an activation of a factory's cell"),
                }
            }
            for sl in &act.slots {
                if let Some(b) = sl.cell {
                    let Some((sc, v)) = self.box_var(b) else {
                        return refuse("a cell without its variable");
                    };
                    self.acts[a].cells.push_out((sl.name.clone(), sc, v));
                    self.owner[sc.0][sc.1][v] = Some((a, sl.name.clone()));
                    self.merged[sc.0][sc.1] = true;
                }
            }
        }
        for a in 0..ir.acts.len() {
            self.dry(a)?;
        }
        for a in 0..ir.acts.len() {
            for (i, b) in self.acts[a].blocks.clone().iter().enumerate() {
                for d in b {
                    let Some(sn) = &d.sname else { continue };
                    if let Some(&(_, sc, v)) = self.acts[a].cells.iter().find(|c| c.0 == *sn) {
                        self.vblock[sc.0][sc.1][v] = Some(Pl::Block(a, i));
                    }
                }
            }
            for &(ref sn, sc, v) in &self.acts[a].cells {
                // A `this` the frame's arrows share is K's own: its body's (0215).
                if matches!(sn.as_str(), "this" | "new.target")
                    && self.vblock[sc.0][sc.1][v].is_none()
                {
                    self.vblock[sc.0][sc.1][v] = Some(Pl::Block(a, 0));
                }
                if self.vblock[sc.0][sc.1][v].is_none() {
                    return Err(cat!("no block of the continuation declares ", sn));
                }
            }
        }
        Ok(())
    }

    fn dry(&mut self, a: usize) -> R<()> {
        let write_k = self.write_k;
        let act = &self.ir.acts[a];
        let mut c = C {
            w: self,
            a,
            dry: true,
        };
        write_k(act, &mut c)?;
        self.acts[a].blocks_done = true;
        Ok(())
    }

    /// A slot's value as continuation text: one that reads a name is evaluated before
    /// the continuation (its own declarations could shadow the name).
    fn slot_value(&mut self, a: usize, name: &str, inline: bool) -> Option<String> {
        let act = &self.ir.acts[a];
        let sl = act.slots.iter().find(|s| s.name == name)?;
        let value = if inline {
            self.inlined_slot(a, name, &sl.value)
        } else {
            sl.value.clone()
        };
        let sl = Slot {
            name: sl.name.clone(),
            value,
            cell: sl.cell,
        };
        // (A name K binds nowhere is the same inside it: a global, a builtin.)
        let scope = self.kscopes[a].clone();
        let r = refs(&sl.value);
        let reads = sl.value.contains("new.target")
            || r.names.iter().any(|n| {
                matches!(n.as_str(), "this" | "arguments" | "super")
                    || n.starts_with("$djs_")
                    || scope.contains(n) && !inline::only_keys(&sl.value, n)
            });
        let st = &mut self.acts[a];
        if !reads || st.after_go.iter().any(|n| n == name) {
            if !inline {
                return Some(sl.value);
            }
            // (Inside K: closures its blocks make may be written here, D3003.)
            return Some(self.props_in_binding(a, name, &sl.value));
        }
        if let Some(p) = st.pre.iter().find(|p| p.0 == name) {
            return Some(p.1.clone());
        }
        let r = cat!("$dump.", act.name, "_", st.pre.len());
        st.pre.push_out((String::from(name), r.clone()));
        Some(r)
    }
}

/// The generator's view of the dump while it writes activation `a`'s continuation.
struct C<'w, 'x> {
    w: &'w mut W<'x>,
    a: usize,
    dry: bool,
}

impl Ctx for C<'_, '_> {
    fn slot(&mut self, name: &str) -> Option<String> {
        if !self.dry && !self.w.vcache.contains_key(&(self.a, String::from(name))) {
            self.w.kept_temp(self.a, name);
        }
        self.w.slot_value(self.a, name, !self.dry)
    }

    fn cell(&mut self, sname: &str) -> Option<Option<String>> {
        let &(_, sc, v) = self.w.acts[self.a].cells.iter().find(|c| c.0 == sname)?;
        let var = &self.w.scope(sc).vars[v];
        if var.tdz {
            return Some(None);
        }
        let init = var.init.clone().unwrap_or_else(|| "undefined".into());
        if self.dry {
            return Some(Some(init));
        }
        // (Its value written in place, as a slot's, D3001, D3003.)
        let key = cat!("<cell>", sname);
        let v = self.w.inlined_slot(self.a, &key, &init);
        Some(Some(self.w.props_in_binding(self.a, &key, &v)))
    }

    fn after_go(&self, sname: &str) -> bool {
        self.w.acts[self.a].after_go.iter().any(|n| n == sname)
    }

    fn rereads(&self, slot: &str) -> bool {
        self.w.ir.acts[self.a].rereads.iter().any(|n| n == slot)
    }

    fn made_in(&self, text: &str) -> Option<usize> {
        let made = &self.w.acts[self.a].made;
        if made.is_empty() {
            return None;
        }
        refs(text)
            .dump
            .iter()
            .filter_map(|(n, _)| made.iter().find(|m| m.0 == *n).map(|m| m.1))
            .max()
    }

    fn arg(&mut self, name: &str) -> Option<String> {
        // (As listed: the call's arguments get their values in place when it is written,
        // outside K, where K's bindings do not matter, D3003.)
        let sl = self.w.ir.acts[self.a]
            .slots
            .iter()
            .find(|s| s.name == name)?;
        Some(sl.value.clone())
    }

    fn block(&mut self, index: usize, info: &[BDecl]) -> R<Vec<String>> {
        if self.dry {
            if !self.w.acts[self.a].blocks_done {
                self.w.acts[self.a].blocks.push_out(info.to_vec());
            }
            return Ok(Vec::new());
        }
        let lines = self.w.emit_place(Pl::Block(self.a, index))?;
        if lines.iter().any(|l| !l.trim().is_empty()) {
            self.w.k_effects = true;
        }
        Ok(lines)
    }

    fn go(&self, block: usize) -> String {
        if self.dry {
            String::new()
        } else {
            self.w.acts[self.a]
                .go
                .get(block)
                .cloned()
                .unwrap_or_default()
        }
    }

    fn site_reads(&self) -> Vec<String> {
        if self.dry {
            return Vec::new();
        }
        self.w.acts[self.a].site_reads.clone()
    }

    fn awaiting(&mut self, scope: &[String]) -> String {
        let act = &self.w.ir.acts[self.a];
        let aw = act.awaiting.clone().unwrap_or_else(|| "undefined".into());
        if self.dry {
            return aw;
        }
        if let Some(v) = self.w.vcache.get(&(self.a, String::from("<awaiting>"))) {
            return v.clone();
        }
        if let Some(v) = self.w.acts[self.a].site_val.clone() {
            self.w
                .vcache
                .insert((self.a, String::from("<awaiting>")), v.clone());
            return v;
        }
        // The site's operand is the first thing K's line evaluates where K is called on
        // its line and does nothing before its site (D3001).
        let a = self.a;
        // (A continuation of a factory's too: its call line calls it, `$djs.k`'s result,
        // D3003.)
        let first = (act.kind == crate::interp::run::suspend::ASYNC
            && self.w.kline == Some(a)
            && !self.w.k_effects
            && self.w.acts[a].go.iter().all(String::is_empty))
        .then_some(0);
        let v = self.w.inline_site(&aw, first, scope);
        self.w
            .vcache
            .insert((a, String::from("<awaiting>")), v.clone());
        v
    }
}

/// What a top-level line of the format 2 text is.
fn classify(l: &str) -> IK {
    if let Some(rest) = l.strip_prefix("$dump.") {
        let (name, after) = ident_at(rest, 0);
        if !name.is_empty() && rest[after..].starts_with(" = ") {
            return IK::Def(String::from(name));
        }
    }
    if declared_fn_name(l).is_some() {
        return IK::FnDecl;
    }
    for kw in ["let ", "const ", "var ", "class "] {
        if let Some(rest) = l.strip_prefix(kw) {
            let (name, _) = ident_at(rest, 0);
            return IK::Decl(alloc::vec![String::from(name)]);
        }
    }
    IK::Stmt
}

fn declared_fn_name(l: &str) -> Option<String> {
    let rest = l.strip_prefix("async ").unwrap_or(l);
    let rest = rest.strip_prefix("function")?;
    let rest = rest.strip_prefix('*').unwrap_or(rest).trim_start();
    let (name, _) = ident_at(rest, 0);
    (!name.is_empty()).then(|| String::from(name))
}

// ---------------------------------------------------------------- placement

impl W<'_> {
    fn parent_of(&mut self, pl: Pl) -> R<Option<Pl>> {
        Ok(match pl {
            Pl::Block(a, i) if i > 0 => Some(Pl::Block(a, i - 1)),
            Pl::Block(a, _) => Some(self.place_of_act(a)?),
            Pl::Group(sc) => Some(self.place_of_group(sc)?),
            Pl::Fac(sc) => match self.parent_scope(sc) {
                Some(p) => Some(self.factory_place(p)?),
                None => Some(Pl::Top),
            },
            Pl::Top => None,
        })
    }

    fn ancestors(&mut self, pl: Pl) -> R<Vec<Pl>> {
        let mut out = alloc::vec![pl];
        let mut p = pl;
        while let Some(q) = self.parent_of(p)? {
            if out.len() > 10_000 {
                return refuse("cyclic placement");
            }
            out.push_out(q);
            p = q;
        }
        Ok(out)
    }

    /// The innermost of `pls`, which must lie on one chain of places.
    fn innermost(&mut self, pls: &[Pl], what: &str) -> R<Option<Pl>> {
        let mut uniq: Vec<Pl> = Vec::new();
        for &p in pls {
            if !uniq.contains(&p) {
                uniq.push_out(p);
            }
        }
        for &c in &uniq {
            let anc = self.ancestors(c)?;
            if uniq.iter().all(|o| anc.contains(o)) {
                return Ok(Some(c));
            }
        }
        if uniq.is_empty() {
            return Ok(None);
        }
        Err(cat!(what, ": scopes of different frames"))
    }

    fn factory_place(&mut self, sc: Sc) -> R<Pl> {
        if self.is_merged(sc) {
            self.place_of_group(sc)
        } else {
            Ok(Pl::Fac(sc))
        }
    }

    /// The scopes whose variables code of `sc` can capture: it and its merged ancestors.
    fn chain(&self, sc: Sc) -> Vec<Sc> {
        let mut out = Vec::new();
        let mut p = Some(sc);
        while let Some(q) = p {
            if !self.is_merged(q) {
                break;
            }
            out.push_out(q);
            p = self.parent_scope(q);
        }
        out
    }

    fn dead(&self, sc: Sc, v: usize) -> bool {
        self.owner[sc.0][sc.1][v].is_none()
    }

    fn has_dead(&self, sc: Sc) -> bool {
        (0..self.scope(sc).vars.len()).any(|v| self.dead(sc, v))
    }

    /// The place of code of merged scope `sc` that captures `caps`.
    fn place_of_code(&mut self, sc: Sc, caps: &[usize]) -> R<Pl> {
        let chain = self.chain(sc);
        let mut pls = Vec::new();
        for &b in caps {
            let Some((vs, v)) = self.box_var(b) else {
                continue;
            };
            if !chain.contains(&vs) {
                continue;
            }
            match self.vblock[vs.0][vs.1][v] {
                Some(pl) if !self.dead(vs, v) => pls.push_out(pl),
                _ => pls.push_out(Pl::Group(vs)),
            }
        }
        if let Some(pl) = self.innermost(&pls, "a closure")? {
            return Ok(pl);
        }
        // It captures nothing of the merged scopes: the nearest unmerged factory.
        let mut p = Some(sc);
        while let Some(q) = p {
            if !self.is_merged(q) {
                return self.factory_place(q);
            }
            p = self.parent_scope(q);
        }
        Ok(Pl::Top)
    }

    /// The place of the block of merged scope `sc`'s variables no frame takes.
    fn place_of_group(&mut self, sc: Sc) -> R<Pl> {
        if let Some(&(_, pl)) = self.group_place.iter().find(|g| g.0 == sc) {
            return Ok(pl);
        }
        if self.in_progress.contains(&sc) {
            return refuse("cyclic group placement");
        }
        self.in_progress.push_out(sc);
        let chain = self.chain(sc);
        let mut pls = Vec::new();
        for (_, caps) in self.factory_code(sc) {
            let touches = caps.iter().any(|&b| {
                self.box_var(b)
                    .is_some_and(|(vs, v)| vs == sc && self.dead(vs, v))
            });
            if !touches {
                continue;
            }
            for &b in &caps {
                let Some((vs, v)) = self.box_var(b) else {
                    continue;
                };
                // (The group's own live variables count: its block goes where their
                // frame declares them.)
                if !chain.contains(&vs) || vs == sc && self.dead(vs, v) {
                    continue;
                }
                match self.vblock[vs.0][vs.1][v] {
                    Some(pl) if !self.dead(vs, v) => pls.push_out(pl),
                    _ => {
                        let g = self.place_of_group(vs)?;
                        pls.push_out(g);
                    }
                }
            }
        }
        let pl = match self.innermost(&pls, "a dead scope")? {
            Some(pl) => pl,
            None => {
                let mut p = self.parent_scope(sc);
                while let Some(q) = p {
                    if !(self.is_merged(q) && !self.has_dead(q)) {
                        break;
                    }
                    p = self.parent_scope(q);
                }
                match p {
                    None => Pl::Top,
                    Some(q) if self.is_merged(q) => Pl::Group(q),
                    Some(q) => Pl::Fac(q),
                }
            }
        };
        self.in_progress.retain(|&x| x != sc);
        self.group_place.push_out((sc, pl));
        Ok(pl)
    }

    /// The code of a scope that is placed (its closures, its unmerged nested scopes),
    /// with the variables each captures.
    fn factory_code(&self, sc: Sc) -> Vec<(Put, Vec<usize>)> {
        let mut out = Vec::new();
        for (i, p) in self.scope(sc).props.iter().enumerate() {
            out.push_out((Put::Prop(sc, i), p.caps.clone()));
        }
        for (k, child) in self.ir.factories[sc.0].scopes.iter().enumerate() {
            if child.parent == sc.1 && !self.is_merged((sc.0, k)) {
                out.push_out((Put::Sub((sc.0, k)), self.free_caps((sc.0, k))));
            }
        }
        out
    }

    /// What the code of scope `sc` and its nested scopes captures from outside them.
    fn free_caps(&self, sc: Sc) -> Vec<usize> {
        let inside = |w: &Self, b: usize| {
            let mut p = w.box_var(b).map(|x| x.0);
            while let Some(q) = p {
                if q == sc {
                    return true;
                }
                p = w.parent_scope(q);
            }
            false
        };
        let mut out = Vec::new();
        for (k, s) in self.ir.factories[sc.0].scopes.iter().enumerate() {
            let mut p = Some((sc.0, k));
            let mut within = false;
            while let Some(q) = p {
                if q == sc {
                    within = true;
                    break;
                }
                p = self.parent_scope(q);
            }
            if !within {
                continue;
            }
            for pr in &s.props {
                for &b in &pr.caps {
                    if !inside(self, b) && !out.contains(&b) {
                        out.push_out(b);
                    }
                }
            }
        }
        out
    }

    fn code_place(&mut self, put: Put, caps: &[usize]) -> R<Pl> {
        let sc = match put {
            Put::Prop(sc, _) | Put::Sub(sc) => sc,
            _ => return refuse("internal: code_place"),
        };
        let sc = match put {
            Put::Sub(_) => self.parent_scope(sc).unwrap_or(sc),
            _ => sc,
        };
        let touches_dead = caps.iter().any(|&b| {
            self.box_var(b)
                .is_some_and(|(vs, v)| vs == sc && self.dead(vs, v))
        });
        if touches_dead {
            return Ok(Pl::Group(sc));
        }
        self.place_of_code(sc, caps)
    }

    /// Where activation `a`'s continuation is written: where its function is.
    fn place_of_act(&mut self, a: usize) -> R<Pl> {
        if let Some(pl) = self.act_place[a] {
            return Ok(pl);
        }
        let pl = match self.acts[a].home {
            None => Pl::Top,
            Some((sc, _)) if !self.is_merged(sc) => self.factory_place(sc)?,
            Some((sc, p)) => {
                // (Where its member was placed: a class's members go with the class.)
                let placed = self.placed.iter().find_map(|x| match x.1 {
                    Put::Prop(psc, pi) if psc == sc && pi == p => Some(x.0),
                    _ => None,
                });
                match placed {
                    Some(pl) => pl,
                    None => {
                        let caps = self.scope(sc).props[p].caps.clone();
                        self.code_place(Put::Prop(sc, p), &caps)?
                    }
                }
            }
        };
        self.act_place[a] = Some(pl);
        Ok(pl)
    }

    fn place_all(&mut self) -> R<()> {
        let ir = self.ir;
        for (f, fac) in ir.factories.iter().enumerate() {
            for s in 0..fac.scopes.len() {
                let sc = (f, s);
                if !self.is_merged(sc) {
                    continue;
                }
                for (put, caps) in self.factory_code(sc) {
                    let pl = self.code_place(put, &caps)?;
                    self.placed.push_out((pl, put));
                }
                // A class statement goes where the variables its methods read are
                // declared (as a closure does), and the members that name it with it.
                let others = self.scope(sc).other.clone();
                let mut lines: Vec<(String, Vec<String>, Pl)> = Vec::new();
                for l in &others {
                    let name = l
                        .strip_prefix("let ")
                        .and_then(|r| r.split_once(" = "))
                        .map(|x| String::from(x.0))
                        .or_else(|| {
                            // `class C …`: the statement declares the scope's variable.
                            l.strip_prefix("class ")
                                .map(|r| String::from(r.split([' ', '{']).next().unwrap_or("")))
                        });
                    let Some(name) = name else {
                        return refuse("plain dumps: a class statement the exporter cannot place");
                    };
                    let names = refs(l).names;
                    let caps: Vec<usize> = self
                        .scope(sc)
                        .vars
                        .iter()
                        .filter(|v| names.contains(&v.name))
                        .map(|v| v.boxn)
                        .collect();
                    let pl = self.place_of_code(sc, &caps)?;
                    if matches!(pl, Pl::Top | Pl::Fac(_)) {
                        return refuse(
                            "plain dumps: a class in a scope a frame shares that reads none of its variables",
                        );
                    }
                    lines.push_out((name, names, pl));
                }
                // And no higher than a class statement whose local it reads (a subclass's
                // base, `$c25` in `((C27) => class D28 extends C27 {…})($c25)`), 0215.
                let by_name: alloc::collections::BTreeMap<&str, usize> = lines
                    .iter()
                    .enumerate()
                    .map(|(b, x)| (x.0.as_str(), b))
                    .collect();
                let reads: Vec<Vec<usize>> = lines
                    .iter()
                    .enumerate()
                    .map(|(a, x)| {
                        x.1.iter()
                            .filter_map(|n| by_name.get(n.as_str()).copied())
                            .filter(|&b| b != a)
                            .collect()
                    })
                    .collect();
                drop(by_name);
                if reads.iter().any(|r| !r.is_empty()) {
                    for _ in 0..lines.len() {
                        let mut changed = false;
                        for a in 0..lines.len() {
                            if reads[a].is_empty() {
                                continue;
                            }
                            let mut pls = alloc::vec![lines[a].2];
                            pls.extend(reads[a].iter().map(|&b| lines[b].2));
                            if let Some(pl) = self.innermost(&pls, "plain dumps: a subclass")?
                                && pl != lines[a].2
                            {
                                lines[a].2 = pl;
                                changed = true;
                            }
                        }
                        if !changed {
                            break;
                        }
                    }
                }
                for (i, (name, _, pl)) in lines.into_iter().enumerate() {
                    let l = &others[i];
                    for x in 0..self.placed.len() {
                        if let Put::Prop(psc, pi) = self.placed[x].1
                            && psc == sc
                            && refs(&self.scope(sc).props[pi].text).names.contains(&name)
                        {
                            self.placed[x].0 = pl;
                        }
                    }
                    if l.starts_with("class ") {
                        let local = cat!("$djs_c", sc.0, "_", sc.1, "_", i);
                        self.class_exprs.push_out((sc, name.clone(), local));
                    }
                    self.placed.push_out((pl, Put::Other(sc, i)));
                }
                if self.has_dead(sc) {
                    let pl = self.place_of_group(sc)?;
                    self.placed.push_out((pl, Put::Group(sc)));
                }
            }
        }
        for a in 0..ir.acts.len() {
            let pl = self.place_of_act(a)?;
            if !matches!(pl, Pl::Top | Pl::Fac(_)) {
                self.placed.push_out((pl, Put::K(a)));
            }
            self.acts[a].place = Some(pl);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- scheduling

/// Whether items need one target (through dependencies), for many items: the items walks from them visited without
/// reaching it are left out of the later walks (the dependencies do not change meanwhile).
struct Walk {
    /// Per item: 1 known not to reach the target, 2 visited by the current walk.
    mark: Vec<u8>,
    seen: Vec<usize>,
    stack: Vec<usize>,
}

impl Walk {
    fn new(n: usize) -> Self {
        Walk {
            mark: alloc::vec![0; n],
            seen: Vec::new(),
            stack: Vec::new(),
        }
    }

    fn needs(&mut self, items: &[Item], from: usize, target: usize) -> bool {
        self.stack.clear();
        self.stack.push(from);
        let mut found = false;
        while let Some(x) = self.stack.pop() {
            if x == target {
                found = true;
                break;
            }
            if x >= self.mark.len() || self.mark[x] != 0 {
                continue;
            }
            self.mark[x] = 2;
            self.seen.push(x);
            self.stack.extend(items[x].deps.iter().copied());
        }
        let v = if found { 0 } else { 1 };
        for &x in &self.seen {
            self.mark[x] = v;
        }
        self.seen.clear();
        found
    }
}

/// The item that provides each key (the first one listed): by the key's hash, exact (the
/// keys of a hash are chained and compared), 0215: a tree of thousands of string keys
/// was most of looking up a data dump's dependencies.
#[derive(Default)]
struct Providers {
    /// Hash -> the last entry of that hash.
    index: super::Index,
    /// Key, item, the entry before it of the same hash (`u32::MAX`: none).
    entries: Vec<(Key, usize, u32)>,
}

impl Providers {
    fn hash(k: &Key) -> u64 {
        let h = match k {
            Key::Loc(n) => inline::name_hash(n),
            Key::Global(n) => inline::name_hash(n) ^ 0x9E37_79B9_7F4A_7C15,
            Key::Factory(n, key) => {
                inline::name_hash(n).rotate_left(17)
                    ^ inline::name_hash(key)
                    ^ 0x2545_F491_4F6C_DD1D
            }
        };
        h.max(1)
    }

    fn find_by(&self, h: u64, same: impl Fn(&Key) -> bool) -> Option<usize> {
        let mut e = self.index.get(h.max(1))?;
        while e != u32::MAX {
            let x = &self.entries[e as usize];
            if same(&x.0) {
                return Some(e as usize);
            }
            e = x.2;
        }
        None
    }

    fn find(&self, k: &Key) -> Option<usize> {
        self.find_by(Self::hash(k), |x| x == k)
    }

    fn get(&self, k: &Key) -> Option<&usize> {
        self.find(k).map(|e| &self.entries[e].1)
    }

    /// [`Providers::get`] of `Key::Loc(name)`, without making the key.
    fn get_loc(&self, name: &str) -> Option<usize> {
        self.find_by(
            inline::name_hash(name),
            |x| matches!(x, Key::Loc(n) if n == name),
        )
        .map(|e| self.entries[e].1)
    }

    fn get_mut(&mut self, k: &Key) -> Option<&mut usize> {
        self.find(k).map(|e| &mut self.entries[e].1)
    }

    /// Adds `k`'s item unless it has one.
    fn or_insert(&mut self, k: Key, item: usize) {
        if self.find(&k).is_some() {
            return;
        }
        let h = Self::hash(&k);
        let prev = self.index.get(h).unwrap_or(u32::MAX);
        self.index.insert(h, self.entries.len() as u32);
        self.entries.push_out((k, item, prev));
    }

    fn insert(&mut self, k: Key, item: usize) {
        match self.get_mut(&k) {
            Some(d) => *d = item,
            None => self.or_insert(k, item),
        }
    }
}

/// A dependency key: `$dump.name`, a key of a merged factory (`name.key`), a global.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Loc(String),
    Factory(String, String),
    Global(String),
}

impl W<'_> {
    fn keys_of(&self, text: &str) -> Vec<Key> {
        self.keys_in(text, true)
    }

    /// The keys of `text`; with `globals` false only its `$dump` references: the
    /// globals a function's text names are read when it runs, after the load.
    fn keys_in(&self, text: &str, globals: bool) -> Vec<Key> {
        if !globals && !has_dollar(text, "$dump.") {
            return Vec::new();
        }
        // (A word's first byte and length filter before the set: most words of a data
        // literal are its keys, 0215.)
        // (The references as `scan` meets them, repeated ones too: their users take the
        // first of each, which a set per text cost more than, 0215.)
        let mut out = Vec::new();
        let b = text.as_bytes();
        let r = scan(
            text,
            true,
            &|n| {
                globals
                    && n.len() <= self.global_max
                    && self.global_first[usize::from(n.as_bytes()[0] >> 6)]
                        & (1u64 << (n.as_bytes()[0] & 63))
                        != 0
                    && self.global_hashes.contains_key(&inline::name_hash(n))
                    && self.global_set.contains(n)
            },
            &mut |s, e| {
                let name = &text[s + 6..e];
                let key = if b.get(e) == Some(&b'.') {
                    ident_at(text, e + 1).0
                } else {
                    ""
                };
                out.push_out(if !key.is_empty() && self.fac_by_name.contains_key(name) {
                    Key::Factory(String::from(name), String::from(key))
                } else {
                    Key::Loc(String::from(name))
                });
            },
            false,
            false,
        );
        // (`scan` gives each global once.)
        for n in r.names {
            out.push_out(Key::Global(n));
        }
        out
    }

    /// The activation whose continuation makes key `key` of factory `name`; `Ok(None)`
    /// when the factory makes it.
    fn creator_of(&mut self, name: &str, key: &str) -> R<Option<usize>> {
        Ok(self.creator_block(name, key)?.map(|x| x.0))
    }

    /// [`W::creator_of`] and the block of its continuation the key is made in.
    fn creator_block(&mut self, name: &str, key: &str) -> R<Option<(usize, usize)>> {
        let ir = self.ir;
        let Some(&f) = self.fac_by_name.get(name) else {
            return Ok(None);
        };
        let mut pl = None;
        for (s, sc) in ir.factories[f].scopes.iter().enumerate() {
            if let Some(p) = sc.props.iter().position(|p| p.key == key) {
                let caps = sc.props[p].caps.clone();
                // (Where it was placed, if it was: a class's members go with the class.)
                let placed = self.placed.iter().find_map(|x| match x.1 {
                    Put::Prop(psc, pi) if psc == (f, s) && pi == p => Some(x.0),
                    _ => None,
                });
                pl = Some(if let Some(pl) = placed {
                    pl
                } else if self.is_merged((f, s)) {
                    self.code_place(Put::Prop((f, s), p), &caps)?
                } else {
                    self.factory_place((f, s))?
                });
            }
        }
        if pl.is_none()
            && let Some(a) = self.kkeys.iter().position(|k| k == key)
        {
            pl = self.acts[a].place;
        }
        let mut p = pl;
        let mut n = 0;
        while let Some(q) = p {
            n += 1;
            if n > 10_000 {
                return refuse("cyclic placement");
            }
            match q {
                Pl::Block(a, b) => return Ok(Some((a, b))),
                Pl::Fac(sc) if self.parent_scope(sc).is_none() => return Ok(None),
                _ => {}
            }
            p = self.parent_of(q)?;
        }
        Ok(None)
    }

    fn act_item(&self, a: usize) -> usize {
        self.act_items.get(a).copied().unwrap_or(usize::MAX)
    }

    fn dep_items(&mut self, keys: &[Key], me: usize, providers: &Providers) -> R<Vec<usize>> {
        let mut out = Vec::new();
        // (Each item once: marked with this call's stamp, 0215.)
        self.dep_stamp += 1;
        let stamp = self.dep_stamp;
        if self.dep_mark.len() < self.items.len() {
            self.dep_mark.resize(self.items.len(), 0);
        }
        for k in keys {
            let d = match k {
                Key::Factory(n, key) => {
                    let merged_key = self.fac_by_name.get(n).is_some_and(|&f| {
                        (0..self.ir.factories[f].scopes.len()).any(|s| self.is_merged((f, s)))
                    });
                    let loc = || providers.get_loc(n);
                    if merged_key {
                        match self.creator_of(n, key)? {
                            Some(a) => Some(self.act_item(a)),
                            None => loc(),
                        }
                    } else {
                        loc()
                    }
                }
                k => providers.get(k).copied(),
            };
            if let Some(d) = d
                && d != me
                && d != usize::MAX
                && self.dep_mark[d] != stamp
            {
                self.dep_mark[d] = stamp;
                out.push_out(d);
            }
        }
        Ok(out)
    }

    /// The text of what is placed at `pl` and below (for dependencies).
    fn placed_text(&self, pl: Pl, out: &mut String, depth: u32) {
        if depth > 64 {
            return;
        }
        for &(p, put) in &self.placed {
            if p != pl {
                continue;
            }
            match put {
                Put::Prop(sc, i) => {
                    cat_to!(out, self.scope(sc).props[i].text, "\n");
                }
                Put::Other(sc, i) => {
                    cat_to!(out, self.scope(sc).other[i], "\n");
                }
                Put::Sub(sc) => self.scope_text(sc, out),
                Put::Group(sc) => {
                    for (v, var) in self.scope(sc).vars.iter().enumerate() {
                        if self.dead(sc, v) {
                            cat_to!(out, var.line, "\n");
                        }
                    }
                    self.placed_text(Pl::Group(sc), out, depth + 1);
                }
                Put::K(_) => {}
            }
        }
    }

    /// What a factory's text evaluates when it runs: its class statements and the
    /// values of its variables and members that are not functions.
    fn scope_eager(&self, sc: Sc, out: &mut String) {
        for (k, s) in self.ir.factories[sc.0].scopes.iter().enumerate() {
            if k != sc.1 && s.parent == sc.1 && !self.is_merged((sc.0, k)) {
                self.scope_eager((sc.0, k), out);
            }
        }
        let s = self.scope(sc);
        for l in &s.other {
            cat_to!(out, l, "\n");
        }
        for v in &s.vars {
            if let Some(init) = &v.init
                && !v.tdz
                && !is_fn_text(init)
            {
                cat_to!(out, init, "\n");
            }
        }
        for p in &s.props {
            // `key: expr` (not `key`, a method or a function).
            if let Some(e) = p
                .text
                .strip_prefix(p.key.as_str())
                .and_then(|t| t.strip_prefix(": "))
                && !is_fn_text(e)
            {
                cat_to!(out, e, "\n");
            }
        }
    }

    fn scope_text(&self, sc: Sc, out: &mut String) {
        for (k, s) in self.ir.factories[sc.0].scopes.iter().enumerate() {
            if k == sc.1 || s.parent == sc.1 && !self.is_merged((sc.0, k)) {
                if k != sc.1 {
                    self.scope_text((sc.0, k), out);
                    continue;
                }
                for l in &s.other {
                    cat_to!(out, l, "\n");
                }
                for v in &s.vars {
                    cat_to!(out, v.line, "\n");
                }
                for p in &s.props {
                    cat_to!(out, p.text, "\n");
                }
            }
        }
    }

    fn schedule(&mut self) -> R<Vec<Plan>> {
        let ir = self.ir;
        let mut list: Vec<(Key, usize)> = Vec::new();
        for (i, it) in self.items.iter().enumerate() {
            match &it.kind {
                IK::Def(n) => list.push_out((Key::Loc(n.clone()), i)),
                IK::Factory(f) => list.push_out((Key::Loc(ir.factories[*f].name.clone()), i)),
                IK::Act(a) => list.push_out((Key::Loc(ir.acts[*a].name.clone()), i)),
                IK::Decl(names) => {
                    for n in names {
                        list.push_out((Key::Global(n.clone()), i));
                    }
                }
                IK::FnDecl | IK::Stmt => {}
            }
        }
        let mut providers: Providers = Default::default();
        for (k, i) in list {
            providers.or_insert(k, i);
        }
        // An async generator's queued requests are made again on its continuation:
        // their promises come from the activation, not their placeholder definitions.
        let mut skip: Vec<usize> = Vec::new();
        for (a, act) in ir.acts.iter().enumerate() {
            let ai = self.act_item(a);
            for (_, _, p) in &act.queue {
                let Some(name) = p.strip_prefix("$dump.") else {
                    return refuse("a queued request's promise");
                };
                if let Some(d) = providers.get_mut(&Key::Loc(name.into())) {
                    let di = *d;
                    if let IK::Def(_) = self.items[di].kind {
                        if !self.items[di].text.ends_with("= new Promise(() => {});") {
                            return refuse("a queued request's promise is not a placeholder");
                        }
                        skip.push_out(di);
                    }
                    *d = ai;
                } else {
                    providers.insert(Key::Loc(name.into()), ai);
                }
            }
        }
        // The dependencies of every item.
        for i in 0..self.items.len() {
            let kind = self.items[i].kind.clone();
            let (deps, start, slots) = match kind {
                IK::Act(a) => {
                    let act = &ir.acts[a];
                    // (Each slot's value scanned once: its keys are its own deps too, 0215.)
                    let slot_keys: Vec<(usize, Vec<Key>)> = act
                        .slots
                        .iter()
                        .enumerate()
                        .filter(|(_, sl)| sl.cell.is_none())
                        .map(|(j, sl)| (j, self.keys_of(&sl.value)))
                        .collect();
                    let mut text = String::new();
                    cat_to!(&mut text, act.this, "\n");
                    // Its origin's function (`K.$djs.f`) and the generator's prototype.
                    cat_to!(&mut text, act.fexpr, "\n");
                    cat_to!(&mut text, act.proto.as_deref().unwrap_or(""), "\n");
                    cat_to!(&mut text, act.super_proto.as_deref().unwrap_or(""), "\n");
                    // Function texts: their globals are read when they run.
                    let mut lazy = String::new();
                    for &(_, sc, v) in &self.acts[a].cells {
                        let var = &self.scope(sc).vars[v];
                        if !var.tdz {
                            let init = var.init.as_deref().unwrap_or("");
                            let t = if is_fn_text(init) {
                                &mut lazy
                            } else {
                                &mut text
                            };
                            cat_to!(t, init, "\n");
                        }
                    }
                    for b in 0..self.acts[a].blocks.len() {
                        self.placed_text(Pl::Block(a, b), &mut lazy, 0);
                    }
                    // (As one scan of the slots' values and `text` lists them: references
                    // in order, then globals.)
                    let rest = self.keys_of(&text);
                    let mut keys: Vec<Key> = Vec::new();
                    let mut seen: alloc::collections::BTreeSet<Key> = Default::default();
                    for global in [false, true] {
                        for k in slot_keys.iter().flat_map(|x| x.1.iter()).chain(&rest) {
                            if matches!(k, Key::Global(_)) == global && seen.insert(k.clone()) {
                                keys.push_out(k.clone());
                            }
                        }
                    }
                    for k in self.keys_in(&lazy, false) {
                        if seen.insert(k.clone()) {
                            keys.push_out(k);
                        }
                    }
                    let mut deps = self.dep_items(&keys, i, &providers)?;
                    let n0 = deps.len();
                    // Its continuation's definition.
                    match self.acts[a].place {
                        Some(Pl::Fac(sc)) => {
                            let top = (sc.0, 0);
                            if sc != top && self.is_merged(top) {
                                // Its factory is written in a continuation's block: that
                                // activation (else the one making its first member).
                                let mut found = None;
                                let sub = self.placed.iter().find_map(|x| match x.1 {
                                    Put::Sub(s2) if s2 == sc => Some(x.0),
                                    _ => None,
                                });
                                let mut p = match sub {
                                    Some(pl) => Some(pl),
                                    None => self.parent_of(Pl::Fac(sc))?,
                                };
                                let mut n = 0;
                                while let Some(q) = p {
                                    n += 1;
                                    if n > 10_000 {
                                        return refuse("cyclic placement");
                                    }
                                    if let Pl::Block(b, _) = q {
                                        found = Some(b);
                                        break;
                                    }
                                    p = self.parent_of(q)?;
                                }
                                if found.is_none() {
                                    let name = ir.factories[sc.0].name.clone();
                                    let key = self.scope(sc).props.first().map(|p| p.key.clone());
                                    if let Some(key) = key {
                                        found = self.creator_of(&name, &key)?;
                                    }
                                }
                                if let Some(c) = found {
                                    deps.push_out(self.act_item(c));
                                }
                            } else if let Some(p) = providers.get_loc(&ir.factories[sc.0].name) {
                                deps.push_out(p);
                            }
                        }
                        Some(pl) if pl != Pl::Top => {
                            let mut p = Some(pl);
                            while let Some(q) = p {
                                if let Pl::Block(b, _) = q {
                                    deps.push_out(self.act_item(b));
                                    break;
                                }
                                p = self.parent_of(q)?;
                            }
                        }
                        _ => {}
                    }
                    self.items[i].def_deps = deps[n0..].to_vec();
                    let start = match &act.awaiting {
                        Some(t) => {
                            let k = self.keys_of(t);
                            self.dep_items(&k, i, &providers)?
                        }
                        None => Vec::new(),
                    };
                    let mut slots = Vec::new();
                    for (j, k) in &slot_keys {
                        let name = act.slots[*j].name.clone();
                        slots.push_out((name, self.dep_items(k, i, &providers)?));
                    }
                    (deps, start, slots)
                }
                IK::Factory(f) => {
                    if self.is_merged((f, 0)) {
                        (Vec::new(), Vec::new(), Vec::new())
                    } else {
                        let mut eager = String::new();
                        self.scope_eager((f, 0), &mut eager);
                        let k = self.keys_of(&eager);
                        (self.dep_items(&k, i, &providers)?, Vec::new(), Vec::new())
                    }
                }
                IK::Def(_)
                    if split_def(&self.items[i].text).is_some_and(|(_, rhs)| is_fn_text(rhs)) =>
                {
                    // A function's text: what it names is read when it runs. (A
                    // continuation of an earlier dump names that dump's `$dump`.)
                    (Vec::new(), Vec::new(), Vec::new())
                }
                _ => {
                    let k = self.keys_of(&self.items[i].text);
                    (self.dep_items(&k, i, &providers)?, Vec::new(), Vec::new())
                }
            };
            let it = &mut self.items[i];
            it.deps = deps;
            it.start_deps = start;
            it.slot_deps = slots;
        }
        // matchAll's iterator clones its regexp when made: the regexp's lastIndex (a
        // patch the dump writes later) must be set before.
        for i in 0..self.items.len() {
            let t = &self.items[i].text;
            if let IK::Def(_) = self.items[i].kind
                && let Some(at) = find_dollar(t, "$djs.regexpIterator($dump.")
            {
                let (re, _) = ident_at(t, at + "$djs.regexpIterator($dump.".len());
                let patch = cat!("$dump.", re, ".lastIndex = ");
                if let Some(p) = self
                    .items
                    .iter()
                    .position(|x| x.kind == IK::Stmt && x.text.starts_with(&patch))
                {
                    self.items[p].pure = true;
                    self.items[i].deps.push_out(p);
                }
            }
        }
        for i in 0..self.items.len() {
            let pure = match &self.items[i].kind {
                IK::Factory(_) | IK::Decl(_) | IK::FnDecl => true,
                IK::Def(_) => !effect_marker(&self.items[i].text),
                _ => false,
            };
            self.items[i].pure |= pure;
        }
        // An activation whose dependencies need it (they need the closures its prefix
        // makes): those go into its callback, and the bindings that take them are
        // declared after the callback.
        for i in 0..self.items.len() {
            let IK::Act(a) = self.items[i].kind else {
                continue;
            };
            let deps = self.items[i].deps.clone();
            let defs = self.items[i].def_deps.clone();
            // (The walks of one activation share what they found: an item a walk that
            // did not reach it visited does not reach it, 0215.)
            let mut walk = Walk::new(self.items.len());
            let cyc: Vec<usize> = deps
                .iter()
                .copied()
                .filter(|&d| !defs.contains(&d) && walk.needs(&self.items, d, i))
                .collect();
            if cyc.is_empty() {
                continue;
            }
            self.items[i].deps.retain(|d| !cyc.contains(d));
            let slots = self.items[i].slot_deps.clone();
            for (k, ds) in slots {
                if ds
                    .iter()
                    .any(|&d| cyc.contains(&d) || walk.needs(&self.items, d, i))
                {
                    self.acts[a].after_go.push_out(k);
                }
            }
            self.acts[a].needs_go = true;
        }
        self.pull(&skip)
    }

    /// Pull scheduling: items in the dump's order, each after what it needs; an
    /// activation needed before its own place is called there, with a callback for
    /// what lies between.
    fn pull(&mut self, skip: &[usize]) -> R<Vec<Plan>> {
        let mut s = Sched {
            emitted: alloc::vec![false; self.items.len()],
            busy: alloc::vec![false; self.items.len()],
            stack: Vec::new(),
            out: alloc::vec![Vec::new()],
        };
        for &k in skip {
            s.emitted[k] = true;
        }
        for i in 0..self.items.len() {
            if let IK::Act(a) = self.items[i].kind {
                if let Some(pos) = s.stack.iter().position(|&x| x == i) {
                    if pos != s.stack.len() - 1 {
                        return refuse("crossing callbacks");
                    }
                    let sd = self.items[i].start_deps.clone();
                    self.ensure(&mut s, &sd, i)?;
                    s.stack.pop();
                    let cb = s.out.pop().unwrap_or_default();
                    // The call entry is the last of the enclosing list.
                    if let Some(Plan::Call(_, c)) = s.out.last_mut().and_then(|l| {
                        l.iter_mut()
                            .rev()
                            .find(|p| matches!(p, Plan::Call(x, Some(_)) if *x == a))
                    }) {
                        *c = Some(cb);
                    }
                    continue;
                }
                let deps = self.items[i].deps.clone();
                self.ensure(&mut s, &deps, i)?;
                if self.acts[a].needs_go && !s.emitted[i] {
                    return refuse("a frame needs data made after it");
                }
                let sd = self.items[i].start_deps.clone();
                self.ensure(&mut s, &sd, i)?;
                if s.emitted[i] {
                    continue;
                }
                s.emitted[i] = true;
                s.out
                    .last_mut()
                    .expect("a list")
                    .push_out(Plan::Call(a, None));
                continue;
            }
            let deps = self.items[i].deps.clone();
            self.ensure(&mut s, &deps, i)?;
            if !s.emitted[i] {
                s.emitted[i] = true;
                s.out.last_mut().expect("a list").push_out(Plan::Item(i));
            }
        }
        if !s.stack.is_empty() {
            return refuse("unclosed callback");
        }
        Ok(s.out.pop().unwrap_or_default())
    }

    fn ensure(&mut self, s: &mut Sched, deps: &[usize], upto: usize) -> R<()> {
        for &d in deps {
            if s.emitted[d] {
                continue;
            }
            if s.busy[d] {
                return refuse("cyclic dependencies");
            }
            if let IK::Act(a) = self.items[d].kind {
                // Called now, with a callback for what follows up to its place.
                s.busy[d] = true;
                let dd = self.items[d].deps.clone();
                self.ensure(s, &dd, upto)?;
                s.busy[d] = false;
                if s.emitted[d] {
                    continue;
                }
                s.emitted[d] = true;
                s.out
                    .last_mut()
                    .expect("a list")
                    .push_out(Plan::Call(a, Some(Vec::new())));
                s.stack.push_out(d);
                s.out.push_out(Vec::new());
                continue;
            }
            if d > upto && !self.items[d].pure {
                return refuse("reordering an effect");
            }
            s.busy[d] = true;
            let dd = self.items[d].deps.clone();
            self.ensure(s, &dd, upto)?;
            s.busy[d] = false;
            if !s.emitted[d] {
                s.emitted[d] = true;
                s.out.last_mut().expect("a list").push_out(Plan::Item(d));
            }
        }
        Ok(())
    }
}

struct Sched {
    emitted: Vec<bool>,
    busy: Vec<bool>,
    /// Open activations (items) whose callback is being filled.
    stack: Vec<usize>,
    /// The lists being filled: the top level, then each open callback.
    out: Vec<Vec<Plan>>,
}

// ---------------------------------------------------------------- emission

/// Intrinsics without a global name, as plain expressions.
const INTRINSICS: &[(&str, &str)] = &[
    (
        "%ArrayIteratorPrototype%",
        "Object.getPrototypeOf([][Symbol.iterator]())",
    ),
    (
        "%MapIteratorPrototype%",
        "Object.getPrototypeOf(new Map()[Symbol.iterator]())",
    ),
    (
        "%SetIteratorPrototype%",
        "Object.getPrototypeOf(new Set()[Symbol.iterator]())",
    ),
    (
        "%StringIteratorPrototype%",
        "Object.getPrototypeOf(\"\"[Symbol.iterator]())",
    ),
    (
        "%RegExpStringIteratorPrototype%",
        "Object.getPrototypeOf(/(?:)/[Symbol.matchAll](\"\"))",
    ),
    (
        "%GeneratorFunction.prototype.prototype%",
        "Object.getPrototypeOf(function* () {}).prototype",
    ),
    (
        "%GeneratorPrototype%",
        "Object.getPrototypeOf(function* () {}).prototype",
    ),
    (
        "%AsyncGeneratorPrototype%",
        "Object.getPrototypeOf(async function* () {}).prototype",
    ),
    (
        "%IteratorPrototype%",
        "Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]()))",
    ),
    (
        "%AsyncIteratorPrototype%",
        "Object.getPrototypeOf(Object.getPrototypeOf(async function* () {}).prototype)",
    ),
    (
        "%IteratorHelperPrototype%",
        "Object.getPrototypeOf([].values().map((x) => x))",
    ),
    (
        "%WrapForValidIteratorPrototype%",
        "Object.getPrototypeOf(Iterator.from({ next() {} }))",
    ),
    ("%TypedArray%", "Object.getPrototypeOf(Uint8Array)"),
];

/// A builtin's registry path (D2371) as a plain expression.
fn builtin_expr(path: &str) -> R<String> {
    let (getter, p) = match path.split_once(' ') {
        Some((g @ ("get" | "set"), rest)) => (Some(g), rest),
        _ => (None, path),
    };
    let mut parts: Vec<&str> = p.split('.').collect();
    let base = if parts[0].starts_with('%') {
        match INTRINSICS.iter().find(|x| x.0 == parts[0]) {
            Some(x) => String::from(x.1),
            None => return Err(cat!("plain dumps: the intrinsic ", parts[0])),
        }
    } else {
        String::from(parts[0])
    };
    if parts.len() == 1 {
        return Ok(base);
    }
    let last = parts.pop().unwrap_or_default();
    let mut obj = base;
    for q in &parts[1..] {
        cat_to!(&mut obj, ".", *q);
    }
    let key = match last.strip_prefix("@@") {
        Some(sym) => cat!("Symbol.", sym),
        None => quote(last),
    };
    Ok(match getter {
        Some(g) => cat!("Object.getOwnPropertyDescriptor(", obj, ", ", key, ").", g),
        None => cat!(obj, "[", key, "]"),
    })
}

impl W<'_> {
    fn emit(&mut self, plan: &[Plan], ts: bool) -> R<String> {
        let mut hoisted: Vec<String> = Vec::new();
        self.open_list();
        self.emit_items(plan, 0, &mut hoisted)?;
        let ir = self.ir;
        // The handles, the Script's last evaluation: a value right before them goes there.
        let handles: Vec<(String, String)> = ir
            .handles
            .iter()
            .map(|(n, e)| {
                let first = (ir.handles.len() == 1).then_some(0);
                (n.clone(), self.inline_refs(e, first, false))
            })
            .collect();
        let mut body = self.close_list();
        let named = self.drop_unread(&mut body, &handles)?;
        body.retain(|l| !l.is_empty());
        let mut s = String::from(if ts {
            "\"djs-dump typescript\";\n"
        } else {
            "\"djs-dump\";\n"
        });
        // (No "use strict": djs compiles all code strict, D015; a harness that runs a
        // dump in node puts the directive in front, D2840.)
        // `$djs.seed(lo, hi[, next])` or `$djs.seed(next)`, where needed (D2837).
        match (ir.seed, ir.next_call) {
            (Some((lo, hi)), Some(n)) => {
                cat_to!(&mut s, "$djs.seed(", lo, ", ", hi, ", ", n, ");\n")
            }
            (Some((lo, hi)), None) => cat_to!(&mut s, "$djs.seed(", lo, ", ", hi, ");\n"),
            (None, Some(n)) => cat_to!(&mut s, "$djs.seed(", n, ");\n"),
            (None, None) => {}
        }
        // (The holder, where anything is left in it: single values go where they are
        // used, D3001.)
        let holder = s.len();
        s.push_str("globalThis.$dump = {};\n");
        // The builtins the text reads (a value a continuation reads again from the
        // original's text, D2905, leaves its builtin unread).
        // (The names the text reads, from `drop_unread`'s pass, 0215. A `$dump.bN` in a
        // string would only define a builtin no one reads.)
        let used: Vec<bool> = (0..ir.builtins.len())
            .map(|i| named.contains(&inline::name_hash(&cat!("b", i))))
            .collect();
        for (i, p) in ir.builtins.iter().enumerate() {
            if used[i] {
                cat_to!(&mut s, "$dump.b", i, " = ", builtin_expr(p)?, ";\n");
            }
        }
        // (Where a member is still written on it: closures may all be in place, D3003.
        // The names on `$dump` the text reads, from `drop_unread`'s pass, 0215.)
        for (f, fac) in ir.factories.iter().enumerate() {
            if self.is_merged((f, 0)) && named.contains(&inline::name_hash(&fac.name)) {
                cat_to!(&mut s, "$dump.", fac.name, " = {};\n");
            }
        }
        for it in &self.items {
            if it.kind == IK::FnDecl {
                cat_to!(&mut s, it.text, "\n");
            }
        }
        for h in &hoisted {
            cat_to!(&mut s, h, "\n");
        }
        for l in &body {
            cat_to!(&mut s, l, "\n");
        }
        // The completion value, the last statement: undefined (not the value of the
        // statement before), the one handle named "run" (what Sandbox, pi and the CLI
        // resume), else every handle by name (D2895). The host deletes `$djs` and `$dump`
        // once the Script ran (D2916).
        match &handles[..] {
            [] => s.push_str("void 0;\n"),
            // (In parentheses unless a reference: an object literal or a function is no
            // expression statement.)
            [(name, e)] if name == "run" => {
                let statement = ["{", "function", "class", "async", "let"]
                    .iter()
                    .any(|k| e.starts_with(k));
                if !statement {
                    cat_to!(&mut s, e, ";\n");
                } else {
                    cat_to!(&mut s, "(", e, ");\n");
                }
            }
            hs => {
                let fields: Vec<String> = hs.iter().map(|(n, e)| cat!(quote(n), ": ", e)).collect();
                cat_to!(
                    &mut s,
                    "({ __proto__: null, ",
                    join(&fields, ", "),
                    " });\n"
                );
            }
        }
        if !s[holder + 23..].contains("$dump") {
            s.replace_range(holder..holder + "globalThis.$dump = {};\n".len(), "");
        }
        Ok(s)
    }

    /// The lines of `list` into the list being written (D3001: a value may go where its one
    /// use is, its own line left out).
    fn emit_items(&mut self, list: &[Plan], depth: u32, hoisted: &mut Vec<String>) -> R<()> {
        let ir = self.ir;
        for p in list {
            match p {
                Plan::Call(a, cb) => {
                    if let Some(cb) = cb {
                        let parts = self.split_callback(*a, cb)?;
                        let mut go = Vec::new();
                        let last = parts.iter().rposition(|p| !p.is_empty());
                        for (b, part) in parts.iter().enumerate() {
                            if part.is_empty() {
                                go.push_out(String::new());
                                continue;
                            }
                            self.open_part(part);
                            self.emit_items(part, depth + 1, hoisted)?;
                            if last == Some(b) {
                                self.site_from_callback(*a, b);
                            }
                            let inner = self.close_list();
                            if inner.is_empty() {
                                go.push_out(String::new());
                                continue;
                            }
                            // (Its lines in K where they read nothing K binds, D3002;
                            // there they may name K's bindings and closures, D3003.)
                            if self.callback_in_k(*a, &inner) {
                                let text = join(&inner, "\n");
                                let (text, _) = self.bindings_in_k(*a, b, &text, false);
                                let text = self.props_in_place(*a, b, &text);
                                go.push_out(text);
                                continue;
                            }
                            let name = if b == 0 {
                                cat!("$dump.g", ir.acts[*a].name)
                            } else {
                                cat!("$dump.g", ir.acts[*a].name, "_", b)
                            };
                            go.push_out(cat!(name, "();"));
                            let l = cat!(name, " = () => {\n", join(&inner, "\n"), "\n};");
                            self.put_line(l, false, None);
                        }
                        self.acts[*a].go = go;
                    }
                    let lines = self.call_text(*a)?;
                    let n = lines.len();
                    let me = cat!("$dump.", ir.acts[*a].name);
                    for (i, l) in lines.into_iter().enumerate() {
                        // The activation's call: effectful, and where it is called on
                        // its own line, a candidate for its one use.
                        let last = i + 1 == n;
                        let def = (last && self.single(&ir.acts[*a].name, false))
                            .then(|| {
                                inline::def_rhs(&l).filter(|_| l.starts_with(&cat!(me, " = ")))
                            })
                            .flatten()
                            .map(|r| (ir.acts[*a].name.clone(), String::from(r), true));
                        let effect = last
                            || !l.starts_with("$dump.")
                            || inline::def_rhs(&l).is_some_and(|r| !inline::shape(r));
                        // (A call whose activation nothing names: a statement.)
                        let l = match inline::def_rhs(&l) {
                            Some(r)
                                if n == 1
                                    && l.starts_with(&cat!(me, " = "))
                                    && !self
                                        .inl
                                        .uses
                                        .contains_key(&inline::name_hash(&ir.acts[*a].name)) =>
                            {
                                inline::statement(r)
                            }
                            _ => l,
                        };
                        let bound = self.acts[*a]
                            .k
                            .as_ref()
                            .map(|k| k.top.clone())
                            .unwrap_or_default();
                        self.flush_hooks(&l);
                        self.put_line_binding(l, effect, def, bound);
                    }
                }
                Plan::Item(i) => {
                    let kind = self.items[*i].kind.clone();
                    let text = self.items[*i].text.clone();
                    // (Host calls keep their order, D3004.)
                    let deferred = matches!(&kind, IK::Def(n) if self.single(n, true))
                        && inline::def_rhs(&text).is_some_and(|r| r.starts_with("$djs.resume("));
                    if !deferred && text.contains("$djs.resume(") {
                        self.flush_all_hooks();
                    }
                    match &kind {
                        IK::FnDecl => {}
                        IK::Factory(f) => {
                            if !self.is_merged((*f, 0)) {
                                let t = self.factory_text((*f, 0))?;
                                let l = cat!("$dump.", ir.factories[*f].name, " = ", t, ";");
                                self.flush_hooks(&l);
                                self.put_line(l, false, None);
                            }
                        }
                        IK::Decl(names) if depth > 0 => {
                            // Declared at the top, assigned in the callback: a global
                            // declaration cannot be in a block. `let` and a class's
                            // binding are mutable like `var`, so this is exact (nothing
                            // reads them before the callback runs); a `const` is not.
                            let text = self.inline_refs(&text, None, false);
                            let effect = Self::effectful(&kind, &text);
                            if let Some(rest) = text.strip_prefix("var ") {
                                hoisted.push_out(cat!("var ", names[0], ";"));
                                self.put_line(String::from(rest), effect, None);
                            } else if let Some(rest) = text.strip_prefix("let ") {
                                hoisted.push_out(cat!("let ", names[0], ";"));
                                self.put_line(String::from(rest), effect, None);
                            } else if text.starts_with("class ") {
                                hoisted.push_out(cat!("let ", names[0], ";"));
                                let l = cat!(names[0], " = ", text.trim_end_matches(';'), ";");
                                self.put_line(l, effect, None);
                            } else {
                                return refuse("a global const declaration inside a callback");
                            }
                        }
                        IK::Def(name) => {
                            let first = inline::first_eval(&text);
                            let text = self.inline_refs(&text, first, false);
                            let effect = Self::effectful(&kind, &text);
                            // A value nothing reads: its effect alone, or nothing.
                            if self.inl.uses.get(&inline::name_hash(name)).copied() == Some(1)
                                && let Some(r) = inline::def_rhs(&text)
                            {
                                if effect {
                                    let st = inline::statement(r);
                                    self.put_line(st, true, None);
                                }
                                continue;
                            }
                            // (A reopened host call read once waits for its reader,
                            // D3004.)
                            if let Some(r) = inline::def_rhs(&text)
                                && r.starts_with("$djs.resume(")
                                && self.single(name, true)
                            {
                                let r = String::from(r);
                                self.defer_hook(name.clone(), text, r);
                                continue;
                            }
                            let def = self
                                .single(name, true)
                                .then(|| inline::def_rhs(&text).map(String::from))
                                .flatten()
                                .map(|r| {
                                    let movable = inline::pure(&r);
                                    (name.clone(), r, !movable)
                                });
                            self.put_line(text, effect, def);
                        }
                        _ => {
                            let first = (kind == IK::Stmt)
                                .then(|| inline::first_eval(&text))
                                .flatten();
                            let text = self.inline_refs(&text, first, false);
                            let effect = Self::effectful(&kind, &text);
                            self.put_line(text, effect, None);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Activation `a`'s callback split by the block of its continuation each entry
    /// needs (D2907): an entry that uses what block `b`'s code makes (a closure, a class,
    /// a continuation of the frame's scopes) runs at the end of block `b`, so that the
    /// continuation's loops and bindings around and below `b` already see what it makes.
    /// An entry runs in the part of the latest block it or the entries it depends on need;
    /// statements and calls keep their order among themselves (property order, side
    /// effects).
    fn split_callback(&mut self, a: usize, cb: &[Plan]) -> R<Vec<Vec<Plan>>> {
        let mut items: Vec<Vec<usize>> = Vec::new();
        let mut parts: Vec<usize> = Vec::new();
        let mut ordered = 0;
        for e in cb {
            let mine = self.plan_items(e);
            let mut part = 0;
            let mut deps: Vec<usize> = Vec::new();
            for &i in &mine {
                for k in self.keys_in(&self.items[i].text.clone(), false) {
                    if let Key::Factory(n, key) = k
                        && let Some((b_act, b)) = self.creator_block(&n, &key)?
                        && b_act == a
                    {
                        part = part.max(b);
                    }
                }
                deps.extend(self.items[i].deps.iter().copied());
                deps.extend(self.items[i].start_deps.iter().copied());
            }
            if let Plan::Call(a2, _) = e {
                for k in self.keys_in(&self.kref(*a2), false) {
                    if let Key::Factory(n, key) = k
                        && let Some((b_act, b)) = self.creator_block(&n, &key)?
                        && b_act == a
                    {
                        part = part.max(b);
                    }
                }
            }
            for (j, its) in items.iter().enumerate() {
                if deps.iter().any(|d| its.contains(d)) {
                    part = part.max(parts[j]);
                }
            }
            let keeps_order = match e {
                Plan::Call(..) => true,
                Plan::Item(i) => self.items[*i].kind == IK::Stmt,
            };
            if keeps_order {
                part = part.max(ordered);
                ordered = part;
            }
            items.push_out(mine);
            parts.push_out(part);
        }
        // A binding declared after the callback takes a value its own block's part
        // makes, or an earlier one's.
        let act_item = self.act_item(a);
        if act_item != usize::MAX {
            for (k, ds) in self.items[act_item].slot_deps.clone() {
                if !self.acts[a].after_go.contains(&k) {
                    continue;
                }
                let block = self.acts[a]
                    .blocks
                    .iter()
                    .position(|b| b.iter().any(|d| d.sname.as_deref() == Some(k.as_str())));
                let need = items
                    .iter()
                    .enumerate()
                    .filter(|(_, its)| ds.iter().any(|d| its.contains(d)))
                    .map(|(j, _)| parts[j])
                    .max();
                if let (Some(b), Some(p)) = (block, need)
                    && p > b
                {
                    return refuse(
                        "a binding whose value a deeper block of its continuation makes",
                    );
                }
            }
        }
        let mut made = Vec::new();
        for (its, &p) in items.iter().zip(&parts) {
            for &i in its {
                let name = match &self.items[i].kind {
                    IK::Def(n) => n.clone(),
                    IK::Factory(f) => self.ir.factories[*f].name.clone(),
                    IK::Act(a2) => self.ir.acts[*a2].name.clone(),
                    _ => continue,
                };
                made.push_out((name, p));
            }
        }
        self.acts[a].made = made;
        let n = parts.iter().copied().max().map_or(0, |m| m + 1);
        let mut out: Vec<Vec<Plan>> = (0..n).map(|_| Vec::new()).collect();
        for (e, p) in cb.iter().zip(parts) {
            out[p].push_out(e.clone());
        }
        Ok(out)
    }

    /// The items an entry of a plan stands for: an item, or an activation and what its
    /// callback holds.
    fn plan_items(&self, e: &Plan) -> Vec<usize> {
        match e {
            Plan::Item(i) => alloc::vec![*i],
            Plan::Call(a, cb) => {
                let mut v = alloc::vec![self.act_item(*a)];
                for x in cb.iter().flatten() {
                    v.extend(self.plan_items(x));
                }
                v.retain(|&i| i != usize::MAX);
                v
            }
        }
    }

    fn kref(&self, a: usize) -> String {
        let act = &self.ir.acts[a];
        match self.acts[a].home {
            Some((sc, _)) => cat!("$dump.", self.ir.factories[sc.0].name, ".", self.kkeys[a]),
            None => cat!("$dump.K", act.name),
        }
    }

    /// The origin's function for `K.$djs`: the run's own when the dump reaches it by a
    /// name, else made from its text in place (D3001): a function either way, whose
    /// template the next dump reads the site maps of (origin tracking, D2870).
    fn fref(&self, a: usize) -> (String, Option<String>) {
        let act = &self.ir.acts[a];
        let e = act.fexpr.as_str();
        let reference = e.split('.').all(|p| !p.is_empty() && is_ident(p));
        if reference {
            (String::from(e), None)
        } else {
            // (Written in place: the record is its one use, D3001.)
            (inline::operand(e), None)
        }
    }

    fn call_text(&mut self, a: usize) -> R<Vec<String>> {
        let ir = self.ir;
        let act = &ir.acts[a];
        // (Its site may take an effectful value written in place, D3001.)
        self.site_first(a);
        self.kline = Some(a);
        self.k_effects = false;
        let k = self.ktext(a);
        self.kline = None;
        let k = k?;
        let mut lines = Vec::new();
        for (name, r) in self.acts[a].pre.clone() {
            let v = act
                .slots
                .iter()
                .find(|s| s.name == name)
                .map_or("undefined", |s| s.value.as_str());
            let v = self.inlined_slot(a, &name, v);
            lines.push_out(cat!(r, " = ", v, ";"));
        }
        let kref = self.kref(a);
        let (fref, fdef) = self.fref(a);
        let fref = self.inline_refs(&fref, None, false);
        lines.extend(fdef);
        // The origin, the site, the holes and the tape a re-dump makes K's layout again
        // from (D2913); fields at their default are left out (D2900): `at` when it is the
        // first await, empty `h` and `r`.
        let me = cat!("$dump.", act.name);
        let generator = act.kind == crate::interp::run::suspend::GENERATOR
            || act.kind == crate::interp::run::suspend::ASYNC_GENERATOR;
        // `$djs.k(K, f, at, r, h, prototype)` gives K its origin record and a generator's
        // prototype, and returns it (D2909, D2915): the origin, the site (an await by its
        // number), the tape and the holes a re-dump makes K's layout again from (D2913),
        // and the generator's prototype where it is not `f.prototype`; arguments at their
        // defaults (the first await, no decisions, no holes) are left out at the end.
        let list = |v: &[u32]| -> String {
            let v: Vec<String> = v.iter().map(|x| cat!(*x)).collect();
            cat!("[", join(&v, ","), "]")
        };
        let mut rec: Vec<String> = alloc::vec![fref.clone()];
        rec.push_out(match act.site.name.strip_prefix("await#") {
            Some(n) if n.parse::<u32>().is_ok() => String::from(n),
            _ => quote(&act.site.name),
        });
        // A tape of yes/no decisions is a number, its bits the decisions (the first the
        // lowest; a tape leaves out its trailing zeros).
        rec.push_out(if k.tape.iter().all(|&x| x <= 1) && k.tape.len() <= 53 {
            cat!(k.tape.iter().rev().fold(0u64, |n, &x| n * 2 + u64::from(x)))
        } else {
            list(&k.tape)
        });
        rec.push_out(list(&k.holes));
        let fproto = cat!(act.fexpr, ".prototype");
        let proto = match &act.proto {
            Some(p) if generator && *p != fproto => self.inline_refs(p, None, false),
            _ => String::new(),
        };
        rec.push_out(proto);
        let defaults = ["", "1", "0", "[]", ""];
        while rec.len() > 1 && rec.last().map(String::as_str) == Some(defaults[rec.len() - 1]) {
            rec.pop();
        }
        let rec = join(&rec, ", ");
        // (`$djs.k` returns K: a continuation of a factory's is called on its result,
        // its one use, so that it may be written there, D3003.)
        let kexpr = if self.acts[a].home.is_none() {
            cat!("$djs.k(", k.text, ", ", rec, ")")
        } else {
            cat!("$djs.k(", kref, ", ", rec, ")")
        };
        // (A `yield*` awaiting its inner `return()` too: K is primed to the delegation's
        // yield and gets the original's request, D2887.)
        let at_yield = act.site.name.starts_with("yield#")
            || act.site.name.starts_with("yield*#")
            || (act.site.kind == crate::compiler::SiteKind::YieldStarAwait
                && act.peers.get(1) == Some(&act.site.name));
        // (The continuation's parameters, D2909, their values in place: evaluated before
        // K runs, outside it.)
        let mut k = k;
        for x in &mut k.args {
            let v = self.inline_refs(x, None, false);
            // (A function as an argument needs no parentheses, and stays lazy, D2995.)
            *x = match v.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
                Some(inner) if v != *x && is_fn_text(inner) => String::from(inner),
                _ => v,
            };
        }
        let args = if k.args.is_empty() {
            String::new()
        } else {
            cat!(", ", join(&k.args, ", "))
        };
        let direct = act.this == "undefined" || self.is_arrow(a);
        let this = self.inline_refs(&act.this, None, false);
        if generator {
            if direct {
                lines.push_out(cat!(me, " = ", kexpr, "(", join(&k.args, ", "), ");"));
            } else {
                lines.push_out(cat!(me, " = ", kexpr, ".call(", this, args, ");"));
            }
            if at_yield {
                lines.push_out(cat!(me, ".next();"));
            } else if act.kind != crate::interp::run::suspend::ASYNC_GENERATOR
                || act.queue.is_empty()
            {
                return refuse("a generator at an await without a request");
            }
            for (i, (t, v, p)) in act.queue.iter().enumerate() {
                if i == 0 && !at_yield {
                    lines.push_out(cat!(p, " = ", me, ".next();"));
                } else {
                    lines.push_out(cat!(p, " = ", me, ".", *t, "(", v, ");"));
                }
            }
        } else if direct {
            lines.push_out(cat!(me, " = ", kexpr, "(", join(&k.args, ", "), ");"));
        } else if self.is_arrow(a) {
            lines.push_out(cat!(me, " = (0, ", kexpr, ")(", join(&k.args, ", "), ");"));
        } else {
            lines.push_out(cat!(me, " = ", kexpr, ".call(", this, args, ");"));
        }
        Ok(lines)
    }

    fn is_arrow(&self, a: usize) -> bool {
        self.ir.acts[a].fkind == func_flags::KIND_ARROW
    }

    fn ktext(&mut self, a: usize) -> R<KText> {
        if let Some(k) = &self.acts[a].k {
            return Ok(k.clone());
        }
        let write_k = self.write_k;
        let act = &self.ir.acts[a];
        let k = {
            let mut c = C {
                w: self,
                a,
                dry: false,
            };
            write_k(act, &mut c)?
        };
        for sl in &act.slots {
            if sl.name.contains('@')
                && (sl.name.contains('.') || sl.name.contains('-'))
                && !k.used.contains(&sl.name)
            {
                return Err(cat!("plain dumps: unused slot ", sl.name));
            }
        }
        self.acts[a].k = Some(k.clone());
        Ok(k)
    }

    /// An unmerged scope's factory: its merged nested scopes left out (their code moved
    /// into continuations), the continuations of functions it holds added.
    fn factory_text(&mut self, sc: Sc) -> R<String> {
        let ir = self.ir;
        let fac = &ir.factories[sc.0];
        let scope = &fac.scopes[sc.1];
        let mut lines = alloc::vec![String::from("(() => {")];
        for l in &scope.other {
            lines.push_out(cat!("  ", l));
        }
        let mut props = Vec::new();
        for (k, child) in fac.scopes.iter().enumerate() {
            if child.parent == sc.1 && !self.is_merged((sc.0, k)) {
                let t = self.factory_text((sc.0, k))?;
                lines.push_out(cat!("const $s", k, " = ", t, ";"));
                props.push_out(cat!("...$s", k));
            }
        }
        for v in &scope.vars {
            if !v.tdz {
                lines.push_out(cat!("  ", v.line));
            }
        }
        for p in &scope.props {
            props.push_out(p.text.clone());
        }
        for a in 0..ir.acts.len() {
            if self.acts[a].place == Some(Pl::Fac(sc)) {
                let k = self.ktext(a)?;
                props.push_out(cat!(self.kkeys[a], ": ", k.text));
            }
        }
        lines.push_out(cat!("  return { ", join(&props, ", "), " };"));
        for v in &scope.vars {
            if v.tdz {
                lines.push_out(cat!("  let ", v.name, ";"));
            }
        }
        lines.push_out(String::from("})()"));
        Ok(join(&lines, "\n"))
    }

    /// Member `i` of scope `sc` as written in a continuation's block (a class statement
    /// written as an expression is named by its local there).
    fn prop_text(&self, sc: Sc, i: usize) -> String {
        let text = self.scope(sc).props[i].text.clone();
        match self
            .class_exprs
            .iter()
            .find(|x| x.0 == sc && (text == x.1 || text == cat!(x.1, ": ", x.1)))
        {
            Some((_, n, local)) => cat!(n, ": ", local),
            None => text,
        }
    }

    /// What the dump writes at a place (a block of a continuation, a dead group).
    fn emit_place(&mut self, pl: Pl) -> R<Vec<String>> {
        let ir = self.ir;
        let mut lines = Vec::new();
        let mut assign: Vec<(String, Vec<String>)> = Vec::new();
        let flush = |assign: &mut Vec<(String, Vec<String>)>, lines: &mut Vec<String>| {
            for (t, ps) in assign.drain(..) {
                lines.push_out(cat!(
                    "Object.assign($dump.",
                    t,
                    ", { ",
                    join(&ps, ", "),
                    " });"
                ));
            }
        };
        let here: Vec<Put> = self
            .placed
            .iter()
            .filter(|x| x.0 == pl)
            .map(|x| x.1)
            .collect();
        // Class statements first, each with the members that name it.
        let mut done: Vec<(Sc, usize)> = Vec::new();
        let mut here = here;
        here.sort_by_key(|p| !matches!(p, Put::Other(..)));
        for put in here {
            match put {
                Put::Other(sc, i) => {
                    flush(&mut assign, &mut lines);
                    let l = self.scope(sc).other[i].clone();
                    // `class C {…}` would declare C again where the frame's binding is:
                    // an expression, its members naming the local.
                    let local = self.class_exprs.iter().find(|x| {
                        x.0 == sc
                            && l.strip_prefix("class ")
                                .is_some_and(|r| r.starts_with(x.1.as_str()))
                    });
                    let named = match local {
                        Some((_, n, local)) => {
                            lines.push_out(cat!("let ", local, " = ", l, ";"));
                            n.clone()
                        }
                        None => {
                            lines.push_out(l.clone());
                            l.strip_prefix("let ")
                                .and_then(|r| r.split_once(" = "))
                                .map(|x| String::from(x.0))
                                .unwrap_or_default()
                        }
                    };
                    // Its members right after it: a later class may extend it.
                    for &(p2, put2) in &self.placed {
                        if p2 != pl {
                            continue;
                        }
                        if let Put::Prop(psc, pi) = put2
                            && psc == sc
                            && refs(&self.scope(sc).props[pi].text).names.contains(&named)
                            && !done.contains(&(psc, pi))
                            && !self.taken.contains(&(psc, pi))
                        {
                            let text = self.prop_text(psc, pi);
                            lines.push_out(cat!(
                                "Object.assign($dump.",
                                ir.factories[sc.0].name,
                                ", { ",
                                text,
                                " });"
                            ));
                            done.push_out((psc, pi));
                        }
                    }
                }
                Put::Prop(..) | Put::K(_) => {
                    let (top, text) = match put {
                        Put::Prop(sc2, i2) => {
                            if done.contains(&(sc2, i2)) || self.taken.contains(&(sc2, i2)) {
                                continue;
                            }
                            (ir.factories[sc2.0].name.clone(), self.prop_text(sc2, i2))
                        }
                        Put::K(a) => {
                            if self.taken_k.contains(&a) {
                                continue;
                            }
                            let Some((hsc, _)) = self.acts[a].home else {
                                return refuse("internal: a placed continuation without a home");
                            };
                            let k = self.ktext(a)?;
                            (
                                ir.factories[hsc.0].name.clone(),
                                cat!(self.kkeys[a], ": ", k.text),
                            )
                        }
                        _ => unreachable!(),
                    };
                    match assign.iter_mut().find(|x| x.0 == top) {
                        Some(x) => x.1.push_out(text),
                        None => assign.push_out((top, alloc::vec![text])),
                    }
                }
                Put::Sub(sc) => {
                    flush(&mut assign, &mut lines);
                    let t = self.factory_text(sc)?;
                    lines.push_out(cat!(
                        "Object.assign($dump.",
                        ir.factories[sc.0].name,
                        ", ",
                        t,
                        ");"
                    ));
                }
                Put::Group(sc) => {
                    flush(&mut assign, &mut lines);
                    let label = cat!("$djs_", ir.factories[sc.0].name, "_", sc.1);
                    let mut inner = Vec::new();
                    let mut tdz = Vec::new();
                    for (v, var) in self.scope(sc).vars.iter().enumerate() {
                        if !self.dead(sc, v) {
                            continue;
                        }
                        if var.tdz {
                            tdz.push_out(var.name.clone());
                        } else {
                            inner.push_out(var.line.clone());
                        }
                    }
                    inner.extend(self.emit_place(Pl::Group(sc))?);
                    if tdz.is_empty() {
                        lines.push_out(cat!("{\n", join(&inner, "\n"), "\n}"));
                    } else {
                        // Declared after a `break` out of the block: never initialized.
                        lines.push_out(cat!(
                            label,
                            ": {\n",
                            join(&inner, "\n"),
                            "\nbreak ",
                            label,
                            ";\nlet ",
                            join(&tdz, ", "),
                            ";\n}"
                        ));
                    }
                }
            }
        }
        flush(&mut assign, &mut lines);
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump_names(r: &Refs) -> Vec<&str> {
        r.dump.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// A regular expression in a template's substitution (0207, seed 9828): the scan once
    /// read it as a division and lost the rest of the text.
    #[test]
    fn spans_after_a_regexp_in_a_substitution() {
        let t = "const v = `${f()}-${g()}:${/a(b)?/g.source}`;\n$dump.zz;";
        assert_eq!(scan_spans(t).len(), 1);
    }

    #[test]
    fn refs_skip_strings_templates_and_properties() {
        let r = refs("$dump.a.b + \"$dump.x\" + `t${$dump.c}` + o.name + /$dump.r/ + n");
        assert_eq!(dump_names(&r), ["a", "c"]);
        assert_eq!(r.dump[0].1.as_deref(), Some("b"));
        assert_eq!(r.names, ["o", "n"]);
    }

    #[test]
    fn eager_refs_skip_function_bodies() {
        let t = "{ f: async () => { $dump.x; g; }, h: (a) => $dump.y + a, k: $dump.z, \
                 m() { return $dump.w; }, n: function (q) { $dump.v }, c: class extends B { m() { $dump.u } } }";
        let r = eager_refs(t);
        assert_eq!(dump_names(&r), ["z"]);
        assert!(r.names.contains(&String::from("B")));
        assert!(!r.names.contains(&String::from("g")));
        let all = refs(t);
        assert_eq!(dump_names(&all), ["x", "y", "z", "w", "v", "u"]);
        // An arrow's expression body ends at its list's comma or bracket.
        let r = eager_refs("[(x) => x.$dump, $dump.a, [y => $dump.b], $dump.c]");
        assert_eq!(dump_names(&r), ["a", "c"]);
        // A call's parenthesized arguments are no function head.
        let r = eager_refs("f($dump.a)({ k: $dump.b })");
        assert_eq!(dump_names(&r), ["a", "b"]);
    }

    #[test]
    fn function_texts() {
        assert!(is_fn_text("async () => 1"));
        assert!(is_fn_text("function* g() {}"));
        assert!(is_fn_text("x => x"));
        assert!(!is_fn_text("($dump.a)"));
        assert!(!is_fn_text("$dump.a"));
    }
}
