//! The hand-written lexer and recursive-descent parser.
//!
//! There is no separate token stream: bash's lexing depends on where the parser is
//! (reserved words only in command position, assignments only before the command name,
//! `$(…)` parsed recursively while a word is being read), so the parser reads the
//! source directly through one cursor and decides at each point what it expects.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use alloc::collections::BTreeMap;

use crate::alias::{AliasUse, Aliases};
use crate::ast::*;
use crate::brace::brace_pass;
use crate::error::ParseError;
use crate::source::Source;
use crate::span::Span;

/// Parser settings that change what is valid syntax.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `shopt -s extglob`: `?(…)`, `*(…)`, `+(…)`, `@(…)` and `!(…)` are patterns in
    /// words and case patterns. They are always patterns on the right of `==` and `!=`
    /// in `[[ ]]`, as in bash.
    pub extglob: bool,
}

/// Parses a whole script with default options.
pub fn parse(source: &str) -> Result<Ast, ParseError> {
    parse_with(source, Options::default())
}

/// Parses a whole script.
pub fn parse_with(source: &str, opts: Options) -> Result<Ast, ParseError> {
    parse_source(Source::from_text(source), opts)
}

/// Parses a script given as bytes, which need not be UTF-8; spans are offsets into
/// `source` (see [`Source`]).
pub fn parse_bytes(source: &[u8], opts: Options) -> Result<Ast, ParseError> {
    parse_source(Source::from_bytes(source), opts)
}

/// Parses a whole text that need not be UTF-8, expanding `aliases` (`None`: no alias
/// expansion) the way bash does while it reads, for text the shell parses at run time
/// in one go (a backquoted command). Aliases defined inside the text do not apply to
/// it; text read line by line (`eval`, `source`) goes through a [`Parser`] instead.
pub fn parse_bytes_with_aliases(
    source: &[u8],
    opts: Options,
    aliases: Option<&Aliases>,
) -> Result<Ast, ParseError> {
    let mut p = Parser::from_bytes(source, opts);
    p.whole = true;
    p.set_aliases(aliases);
    match p.next_chunk() {
        Some(r) => r,
        None => parse_bytes(source, opts),
    }
}

fn parse_source(source: Source, opts: Options) -> Result<Ast, ParseError> {
    let mut p = P::new(source.bytes(), 0, source.bytes().len(), opts, None)?;
    let mut items = Vec::new();
    while p.parse_line(&mut items)? {}
    p.finish_pending();
    Ok(p.into_ast(items, source.clone()))
}

/// A table's alias texts after the script: (table, buffer, name to text range).
type AliasBuf = (Aliases, Arc<[u8]>, BTreeMap<Vec<u8>, (usize, usize)>);

/// Finds an alias text in the chunk's buffer: by name and the number of the use.
type AliasLookup<'a> = &'a dyn Fn(&[u8], usize) -> Option<(usize, usize)>;

/// An alias expansion being read: where to return to in the enclosing text.
struct Seg {
    ret: usize,
    end: usize,
    name: Vec<u8>,
    /// The text ends in a blank: the word after it is checked for an alias too.
    trailing_blank: bool,
}

/// Parses a script one complete top-level command line at a time, as bash runs scripts:
/// each chunk is parsed (and can be run) before the next one is read, so a syntax error
/// further down does not prevent the commands before it from running, and the options
/// can change between chunks (`shopt -s extglob`).
#[derive(Clone, Debug)]
pub struct Parser {
    source: Source,
    pos: usize,
    opts: Options,
    done: bool,
    aliases: Option<Aliases>,
    /// One chunk for the whole input (`parse_bytes_with_aliases`).
    whole: bool,
    /// The script followed by every alias text once, for the table in `aliases`.
    alias_buf: Option<AliasBuf>,
}

impl Parser {
    pub fn new(source: impl Into<Arc<str>>, opts: Options) -> Parser {
        Parser::with_source(Source::from_text(source), opts)
    }

    /// A parser over bytes that need not be UTF-8 (see [`Source`]).
    pub fn from_bytes(source: &[u8], opts: Options) -> Parser {
        Parser::with_source(Source::from_bytes(source), opts)
    }

    fn with_source(source: Source, opts: Options) -> Parser {
        Parser {
            source,
            pos: 0,
            opts,
            done: false,
            aliases: None,
            whole: false,
            alias_buf: None,
        }
    }

    pub fn set_options(&mut self, opts: Options) {
        self.opts = opts;
    }

    /// The aliases to expand in the following chunks, as bash does while it reads them
    /// (`None`: `expand_aliases` is off). Set it before each chunk; the table is only
    /// copied into the parser's buffer when it changes.
    pub fn set_aliases(&mut self, aliases: Option<&Aliases>) {
        self.aliases = aliases.filter(|a| !a.is_empty()).cloned();
    }

    /// Byte offset where the next chunk starts.
    pub fn offset(&self) -> usize {
        self.pos
    }

    /// The next chunk, or `None` at the end of the input. After an error, returns
    /// `None`.
    pub fn next_chunk(&mut self) -> Option<Result<Ast, ParseError>> {
        if self.done {
            return None;
        }
        let source = self.source.clone();
        let result = match self.aliases.clone() {
            None => chunk(source.bytes(), self.pos, self.opts, None, self.whole).map(|r| {
                r.map(|(p, items)| {
                    let ast = p.into_ast(items, source.clone());
                    let pos = ast.chunk_end;
                    (ast, pos)
                })
            }),
            Some(aliases) => self.chunk_with_aliases(&aliases),
        };
        match result {
            Ok(Some((ast, pos))) => {
                self.pos = pos;
                Some(Ok(ast))
            }
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

impl Parser {
    /// A chunk parsed with aliases: a first pass finds which aliases it expands, in
    /// which order; a second pass parses it over the script followed by one copy of the
    /// alias text per use, so that every node from an alias has its own span.
    fn chunk_with_aliases(&mut self, aliases: &Aliases) -> R<Option<(Ast, usize)>> {
        let fresh = !matches!(&self.alias_buf, Some((a, _, _)) if a.same(aliases));
        if fresh {
            let mut buf = self.source.bytes().to_vec();
            let mut regions = BTreeMap::new();
            for (name, text) in aliases.iter() {
                let start = buf.len();
                buf.extend_from_slice(text);
                regions.insert(name.to_vec(), (start, buf.len()));
            }
            self.alias_buf = Some((aliases.clone(), Arc::from(buf), regions));
        }
        let (_, buf, regions) = self.alias_buf.as_ref().unwrap();
        let script_len = self.source.bytes().len();
        let by_name = |name: &[u8], _: usize| regions.get(name).copied();
        let first = chunk(
            buf,
            self.pos,
            self.opts,
            Some((script_len, &by_name)),
            self.whole,
        )?;
        let Some((p, items)) = first else {
            return Ok(None);
        };
        if p.uses.is_empty() {
            let ast = p.into_ast(items, self.source.clone());
            let pos = ast.chunk_end;
            return Ok(Some((ast, pos)));
        }
        // The second pass: one copy of the text per use.
        let mut buf2 = self.source.bytes().to_vec();
        let mut plan = Vec::new();
        for u in &p.uses {
            let (s, e) = regions[&u.name];
            let start = buf2.len();
            buf2.extend_from_slice(&buf[s..e]);
            plan.push((u.name.clone(), start, buf2.len()));
        }
        let by_use = |name: &[u8], k: usize| {
            plan.get(k)
                .filter(|(n, _, _)| n.as_slice() == name)
                .map(|&(_, s, e)| (s, e))
        };
        let (p, items) = chunk(
            &buf2,
            self.pos,
            self.opts,
            Some((script_len, &by_use)),
            self.whole,
        )?
        .expect("the second pass reads what the first did");
        let uses: Vec<AliasUse> = p
            .uses
            .iter()
            .map(|u| AliasUse {
                name: u.name.clone(),
                word: u.word,
                text: u.text,
            })
            .collect();
        let source = Source::from_bytes(&buf2);
        let mut ast = p.into_ast(items, source);
        ast.script_len = script_len as u32;
        ast.alias_uses = uses;
        let uses = ast.alias_uses.clone();
        crate::remap::map_spans(&mut ast, &mut |s| join_span(&uses, s));
        let pos = ast.chunk_end;
        Ok(Some((ast, pos)))
    }
}

/// Parses one top-level line (and, when it ends inside alias text, the rest of that
/// text) from `pos`.
fn chunk<'a>(
    buf: &'a [u8],
    pos: usize,
    opts: Options,
    alias: Option<(usize, AliasLookup<'a>)>,
    whole: bool,
) -> R<Option<(P<'a>, Vec<AndOr>)>> {
    let script_len = alias.map_or(buf.len(), |(n, _)| n);
    let mut p = P::new(buf, pos, script_len, opts, alias.map(|(_, l)| l))?;
    let mut items = Vec::new();
    let r = (|| -> R<bool> {
        if !p.parse_line(&mut items)? {
            return Ok(false);
        }
        while whole || !p.segs.is_empty() {
            p.skip_blanks();
            if (!whole && p.segs.is_empty()) || !p.parse_line(&mut items)? {
                break;
            }
        }
        Ok(true)
    })();
    match r {
        Ok(true) => {}
        Ok(false) => return Ok(None),
        Err(mut e) => {
            // Point errors in alias text at the alias word.
            e.span = original_span(&p.uses, e.span);
            return Err(e);
        }
    }
    p.skip_blanks();
    if p.pos >= p.end {
        p.finish_pending();
    }
    p.chunk_end = p.pos;
    Ok(Some((p, items)))
}

/// The offset of `x` lifted out of alias text: the use whose text holds it (for the end
/// of a span, `x` may be the text's end).
fn region(uses: &[AliasUse], x: u32, is_end: bool) -> Option<usize> {
    uses.iter().position(|u| {
        if is_end {
            u.text.start < x && x <= u.text.end
        } else {
            u.text.start <= x && x < u.text.end
        }
    })
}

fn depth(uses: &[AliasUse], mut r: Option<usize>) -> usize {
    let mut d = 0;
    while let Some(k) = r {
        d += 1;
        r = region(uses, uses[k].word.start, false);
    }
    d
}

/// A span that covers alias text and the text around it is widened to the words the
/// aliases replaced; spans inside one alias text stay there.
fn join_span(uses: &[AliasUse], s: Span) -> Span {
    let (mut a, mut b) = (s.start, s.end);
    if a == b {
        return s;
    }
    loop {
        let ra = region(uses, a, false);
        let rb = region(uses, b, true);
        if ra == rb {
            return Span::new(a, b);
        }
        let (da, db) = (depth(uses, ra), depth(uses, rb));
        if da >= db
            && let Some(k) = ra
        {
            a = uses[k].word.start;
        }
        if db >= da
            && let Some(k) = rb
        {
            b = uses[k].word.end;
        }
    }
}

/// A span lifted all the way out of alias text, onto the script.
pub(crate) fn original_span(uses: &[AliasUse], s: Span) -> Span {
    let (mut a, mut b) = (s.start, s.end);
    while let Some(k) = region(uses, a, false).or_else(|| region(uses, a, true)) {
        a = uses[k].word.start;
    }
    while let Some(k) = region(uses, b, true) {
        b = uses[k].word.end;
    }
    Span::new(a, b.max(a))
}

/// Nesting limit (compound lists plus word nesting), so that hostile input fails with
/// an error instead of overflowing the stack: 100 fits wasm32's 1 MiB stack with room
/// to spare in release builds (the `deep_nesting_fits_a_small_stack` test). Real
/// scripts stay far below it.
const MAX_DEPTH: u32 = 100;

pub(crate) const RESERVED: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "case", "esac", "while", "until", "for",
    "select", "function", "in", "time", "{", "}", "!", "[[", "]]", "coproc",
];

/// Commands whose `name=( … )` arguments are parsed as compound assignments (bash's
/// list, probed: `echo a=(1)` is a syntax error, `alias a=(1)` is not).
const DECL_BUILTINS: &[&str] = &[
    "declare", "typeset", "local", "export", "readonly", "alias", "let", "eval",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    AndIf,
    OrIf,
    Semi,
    DSemi,
    SemiAnd,
    DSemiAnd,
    Amp,
    Pipe,
    PipeAmp,
    LParen,
    RParen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ctx {
    /// An unquoted word: ends at blanks and metachars.
    Word,
    /// A word in `[[ ]]`, or a case pattern: like `Word`, with extglob when `ext`.
    Pattern { ext: bool },
    /// The right side of `=~`: parentheses nest and `|` is literal.
    Regex,
    /// Inside `"…"`.
    Double,
    /// An unquoted here-doc body (bounded by `end`).
    HereDoc { strip: bool },
    /// The word of a `${name op word}`; `stop` is an extra terminator (`/`, `:`) or 0.
    Operand { dq: bool, stop: u8 },
    /// Inside `[…]` of a subscript.
    Subscript,
    /// Arithmetic text: `$((…))` and `((…))` (bounded by `end`), or a clause of
    /// `for ((…))` when `semi` (ends at `;`).
    Arith { semi: bool },
    /// Inside `$[…]`.
    ArithBracket,
    /// One alternative of an extglob pattern.
    ExtGlob,
}

impl Ctx {
    /// Backslash and `$'` behave as inside double quotes.
    fn dq(self) -> bool {
        matches!(
            self,
            Ctx::Double
                | Ctx::HereDoc { .. }
                | Ctx::Operand { dq: true, .. }
                | Ctx::Arith { .. }
                | Ctx::ArithBracket
        )
    }
}

#[derive(Clone, Copy)]
enum Term {
    /// The `}` of `${ …; }`, even with more word chars right after it (`${ :; }x`),
    /// unlike the reserved word `}` of a brace group.
    FunSubClose,
    /// `)`
    Paren,
    /// One of these reserved words.
    Words(&'static [&'static str]),
    /// `;;`, `;&`, `;;&` or `esac`.
    Case,
    /// The end of the text (a `$(…)` in an extglob, bounded beforehand).
    End,
}

/// What opened a compound list, for the end-of-file message.
#[derive(Clone, Copy)]
struct Opener {
    what: &'static str,
    pos: usize,
    /// "unexpected EOF while looking for matching" rather than "unexpected end of file
    /// from … command".
    matching: bool,
}

struct Pending {
    id: HereDocId,
    delim: Vec<u8>,
    strip: bool,
    quoted: bool,
    /// Left without a body by the `$(…)` it is in.
    leaked: bool,
}

struct Checkpoint {
    pos: usize,
    lists: usize,
    cmds: usize,
    heredocs: usize,
    pending: usize,
}

type R<T> = Result<T, ParseError>;

struct P<'a> {
    b: &'a [u8],
    pos: usize,
    end: usize,
    /// Where the next chunk starts, once this one is parsed.
    chunk_end: usize,
    alias: Option<AliasLookup<'a>>,
    /// Alias texts being read, innermost last.
    segs: Vec<Seg>,
    uses: Vec<AliasUse>,
    /// The alias text just left ended in a blank: the next word is an alias candidate.
    alias_next: bool,
    opts: Options,
    lists: Vec<List>,
    cmds: Vec<Command>,
    heredocs: Vec<HereDoc>,
    pending: Vec<Pending>,
    depth: u32,
    /// Nesting of `$(…)`, for here-doc delimiters followed by `)`.
    comsub: u32,
    /// Inside the body of an unquoted here-doc, where bash removes line continuations
    /// before anything else, even within single quotes in a `$(…)`.
    heredoc_body: u32,
    /// The closing token of the innermost `$(…)`, `<(…)` or `${ …; }`, for "while
    /// looking for matching" in error messages.
    closer: Option<&'static str>,
}

fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn is_meta(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>'
    )
}

fn is_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn is_special_param(c: u8) -> bool {
    matches!(c, b'@' | b'*' | b'#' | b'?' | b'-' | b'$' | b'!')
}

/// The length of the char at `b[i..end]`: the length of a valid UTF-8 sequence there,
/// else 1 (an undecodable byte stands alone, so spans never split a char of the view).
pub(crate) fn char_len(b: &[u8], i: usize, end: usize) -> usize {
    let n = match b[i] {
        0xF0..=0xF7 => 4,
        0xE0..=0xEF => 3,
        0xC2..=0xDF => 2,
        _ => return 1,
    };
    if i + n <= end && core::str::from_utf8(&b[i..i + n]).is_ok() {
        n
    } else {
        1
    }
}

/// The span from the start of `first` to the end of `last`, which follows it in the
/// input. Not `Span::to`: with aliases, text read later may lie earlier in the buffer.
fn join(first: Span, last: Span) -> Span {
    Span::new(first.start, last.end)
}

fn sp(start: usize, end: usize) -> Span {
    Span::from_usize(start, end)
}

fn push_lit(out: &mut Vec<WordPart>, start: usize, end: usize) {
    if start == end {
        return;
    }
    if let Some(last) = out.last_mut()
        && matches!(last.kind, Part::Lit)
        && last.span.end as usize == start
    {
        last.span.end = end as u32;
        return;
    }
    out.push(WordPart {
        span: sp(start, end),
        kind: Part::Lit,
    });
}

impl<'a> P<'a> {
    fn new(
        src: &'a [u8],
        pos: usize,
        script_len: usize,
        opts: Options,
        alias: Option<AliasLookup<'a>>,
    ) -> R<P<'a>> {
        if src.len() >= u32::MAX as usize {
            return Err(ParseError {
                span: Span::default(),
                message: String::from("script too large"),
                incomplete: false,
            });
        }
        Ok(P {
            b: src,
            pos,
            end: script_len,
            chunk_end: pos,
            alias,
            segs: Vec::new(),
            uses: Vec::new(),
            alias_next: false,
            opts,
            lists: Vec::new(),
            cmds: Vec::new(),
            heredocs: Vec::new(),
            pending: Vec::new(),
            depth: 0,
            comsub: 0,
            heredoc_body: 0,
            closer: None,
        })
    }

    fn into_ast(mut self, items: Vec<AndOr>, source: Source) -> Ast {
        let span = match (items.first(), items.last()) {
            (Some(f), Some(l)) => join(f.span, l.span),
            _ => sp(self.pos, self.pos),
        };
        let root = ListId(self.lists.len() as u32);
        self.lists.push(List { span, items });
        let script_len = source.bytes().len() as u32;
        Ast {
            source,
            lists: self.lists,
            cmds: self.cmds,
            heredocs: self.heredocs,
            root,
            script_len,
            alias_uses: Vec::new(),
            chunk_end: self.chunk_end,
        }
    }

    // ---- cursor ----

    fn peek(&self) -> Option<u8> {
        if self.pos < self.end {
            Some(self.b[self.pos])
        } else {
            None
        }
    }

    fn peek_at(&self, off: usize) -> Option<u8> {
        let i = self.pos + off;
        if i < self.end { Some(self.b[i]) } else { None }
    }

    fn at(&self, s: &[u8]) -> bool {
        self.pos + s.len() <= self.end && &self.b[self.pos..self.pos + s.len()] == s
    }

    fn at_eof(&self) -> bool {
        self.pos >= self.end
    }

    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            pos: self.pos,
            lists: self.lists.len(),
            cmds: self.cmds.len(),
            heredocs: self.heredocs.len(),
            pending: self.pending.len(),
        }
    }

    fn rewind(&mut self, cp: Checkpoint) {
        self.pos = cp.pos;
        self.lists.truncate(cp.lists);
        self.cmds.truncate(cp.cmds);
        self.heredocs.truncate(cp.heredocs);
        self.pending.truncate(cp.pending);
    }

    fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error_here("nesting too deep"));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    // ---- errors ----

    fn error_at(&self, start: usize, end: usize, message: String) -> ParseError {
        ParseError {
            span: sp(start, end),
            message,
            incomplete: false,
        }
    }

    fn error_here(&self, message: &str) -> ParseError {
        self.error_at(self.pos, self.pos, String::from(message))
    }

    fn eof_error(&self, message: String) -> ParseError {
        ParseError {
            span: sp(self.end, self.end),
            message,
            incomplete: true,
        }
    }

    fn unterminated(&self, what: &str) -> ParseError {
        self.eof_error(format!(
            "unexpected EOF while looking for matching `{what}'"
        ))
    }

    /// Source bytes as text for messages (invalid UTF-8 replaced).
    fn lossy(&self, start: usize, end: usize) -> String {
        String::from_utf8_lossy(&self.b[start..end]).into_owned()
    }

    fn line_of(&self, pos: usize) -> usize {
        self.b[..pos].iter().filter(|&&c| c == b'\n').count() + 1
    }

    fn eof_from(&self, opener: Opener) -> ParseError {
        if opener.matching {
            self.unterminated(opener.what)
        } else {
            self.eof_error(format!(
                "syntax error: unexpected end of file from `{}' command on line {}",
                opener.what,
                self.line_of(opener.pos)
            ))
        }
    }

    /// The text of the token at the cursor (blanks already skipped), as bash names it in
    /// "near unexpected token".
    fn token_text(&self) -> (usize, usize) {
        let start = self.pos;
        let Some(c) = self.peek() else {
            return (start, start);
        };
        if c == b'\n' {
            return (start, start + 1);
        }
        if let Some((_, len)) = self.peek_op() {
            return (start, start + len);
        }
        if let Some((_, _, _, len)) = self.peek_redirect() {
            return (start, start + len);
        }
        let mut i = start + self.run().len();
        if i == start {
            i = start + 1;
        }
        (start, i)
    }

    fn unexpected(&self) -> ParseError {
        if self.at_eof() {
            return self.eof_error(String::from("syntax error: unexpected end of file"));
        }
        let (s, e) = self.token_text();
        let lossy = self.lossy(s, e);
        let text: &str = if self.b[s] == b'\n' {
            "newline"
        } else {
            &lossy
        };
        // Inside a substitution bash names what it was looking for, unless the
        // unexpected token is that closer itself.
        let message = match self.closer {
            Some(c) if c != text => {
                format!(
                    "syntax error near unexpected token `{text}' while looking for matching `{c}'"
                )
            }
            _ => format!("syntax error near unexpected token `{text}'"),
        };
        self.error_at(s, e, message)
    }

    /// A redirection without its word: at the end of the input bash names the missing
    /// token `newline` too.
    fn missing_word(&self) -> ParseError {
        if self.at_eof() {
            return self.error_at(
                self.pos,
                self.pos,
                String::from("syntax error near unexpected token `newline'"),
            );
        }
        self.unexpected()
    }

    // ---- aliases ----

    /// At a place where a command word may stand: if the word there is an unquoted
    /// alias name (and that alias is not being expanded already), read its text
    /// instead. Repeats for the first word of the text.
    fn maybe_alias(&mut self) {
        let Some(lookup) = self.alias else {
            self.skip_blanks();
            return;
        };
        loop {
            self.skip_blanks();
            self.alias_next = false;
            let run = self.run();
            let expand = !run.is_empty()
                && !run
                    .iter()
                    .any(|c| matches!(c, b'\'' | b'"' | b'\\' | b'$' | b'`'))
                && !self.segs.iter().any(|s| s.name == run);
            let found = if expand {
                lookup(run, self.uses.len())
            } else {
                None
            };
            let Some((s, e)) = found else {
                return;
            };
            let word = sp(self.pos, self.pos + run.len());
            self.uses.push(AliasUse {
                name: run.to_vec(),
                word,
                text: sp(s, e),
            });
            self.segs.push(Seg {
                ret: word.end as usize,
                end: self.end,
                name: run.to_vec(),
                trailing_blank: e > s && is_blank(self.b[e - 1]),
            });
            self.pos = s;
            self.end = e;
        }
    }

    /// The word after an alias text that ends in a blank is an alias candidate too,
    /// wherever it stands (`alias FOR='for ' eye='i ' IN='in '; FOR eye IN a b`).
    fn alias_after_blank(&mut self) {
        if core::mem::take(&mut self.alias_next) {
            self.maybe_alias();
        }
    }

    // ---- blanks, comments, newlines, here-docs ----

    /// Skips blanks, line continuations and a comment, not newlines.
    fn skip_blanks(&mut self) {
        loop {
            // The end of an alias text is a token boundary: back to the text after
            // the alias word.
            if self.pos >= self.end
                && let Some(seg) = self.segs.pop()
            {
                self.pos = seg.ret;
                self.end = seg.end;
                if seg.trailing_blank {
                    self.alias_next = true;
                }
                continue;
            }
            match self.peek() {
                Some(c) if is_blank(c) => self.pos += 1,
                Some(b'\\') if self.peek_at(1) == Some(b'\n') => self.pos += 2,
                Some(b'#') => loop {
                    match self.peek() {
                        Some(b'\n') => break,
                        Some(_) => self.pos += 1,
                        // A comment in alias text runs on into the line after the
                        // alias word, as bash reads the text as input.
                        None => match self.segs.pop() {
                            Some(seg) => {
                                self.pos = seg.ret;
                                self.end = seg.end;
                            }
                            None => break,
                        },
                    }
                },
                _ => return,
            }
        }
    }

    /// Skips blanks, comments and newlines, reading here-doc bodies at each newline.
    fn skip_linebreaks(&mut self) -> R<()> {
        loop {
            self.skip_blanks();
            if self.peek() == Some(b'\n') {
                self.newline()?;
            } else {
                return Ok(());
            }
        }
    }

    fn newline(&mut self) -> R<()> {
        debug_assert_eq!(self.peek(), Some(b'\n'));
        self.pos += 1;
        if !self.pending.is_empty() {
            self.read_heredoc_bodies()?;
        }
        Ok(())
    }

    fn read_heredoc_bodies(&mut self) -> R<()> {
        let pending = core::mem::take(&mut self.pending);
        for p in pending {
            let mut error = None;
            let body_start = self.pos;
            let mut body_end = self.end;
            let mut terminated = false;
            while self.pos < self.end {
                let line_start = self.pos;
                let mut line_end = line_start;
                while line_end < self.end && self.b[line_end] != b'\n' {
                    line_end += 1;
                }
                let mut content = line_start;
                if p.strip {
                    while content < line_end && self.b[content] == b'\t' {
                        content += 1;
                    }
                }
                let line = &self.b[content..line_end];
                if line == &p.delim[..] {
                    body_end = line_start;
                    self.pos = (line_end + 1).min(self.end);
                    terminated = true;
                    break;
                }
                // Inside `$(…)`, bash ends the body at a delimiter directly followed by
                // the closing parenthesis (and warns as at end of file).
                if self.comsub > 0
                    && line.starts_with(&p.delim)
                    && line.get(p.delim.len()) == Some(&b')')
                {
                    body_end = line_start;
                    self.pos = content + p.delim.len();
                    break;
                }
                self.pos = (line_end + 1).min(self.end);
            }
            if !terminated && body_end == self.end {
                self.pos = self.end;
            }
            let after = self.pos;
            let mut parts = Vec::new();
            if p.quoted {
                let mut i = body_start;
                while i < body_end {
                    let mut s = i;
                    if p.strip {
                        while s < body_end && self.b[s] == b'\t' {
                            s += 1;
                        }
                    }
                    let mut e = s;
                    while e < body_end && self.b[e] != b'\n' {
                        e += 1;
                    }
                    let e = (e + 1).min(body_end);
                    if s < e {
                        parts.push(WordPart {
                            span: sp(s, e),
                            kind: Part::Lit,
                        });
                    }
                    i = e;
                }
            } else {
                let saved_end = self.end;
                self.end = body_end;
                self.pos = body_start;
                if p.strip {
                    while self.peek() == Some(b'\t') {
                        self.pos += 1;
                    }
                }
                // Bash parses a here-doc body only when it expands it: an error in it
                // is reported then, not as a syntax error of the script.
                let cp = self.checkpoint();
                let (depth, comsub) = (self.depth, self.comsub);
                self.heredoc_body += 1;
                let r = self.parse_parts(Ctx::HereDoc { strip: p.strip }, &mut parts);
                self.heredoc_body -= 1;
                if let Err(e) = r {
                    self.rewind(cp);
                    (self.depth, self.comsub) = (depth, comsub);
                    parts.clear();
                    push_lit(&mut parts, body_start, body_end);
                    error = Some(e);
                }
                self.end = saved_end;
            }
            self.pos = after;
            let doc = &mut self.heredocs[p.id.index()];
            doc.body = Word {
                span: sp(body_start, body_end),
                parts,
            };
            doc.terminated = terminated;
            doc.error = error;
        }
        Ok(())
    }

    /// Here-docs of the enclosing command line wait while a nested `$(…)` is parsed:
    /// their bodies start after the newline that ends that command line, not after a
    /// newline inside the substitution.
    fn stash_pending(&mut self) -> Vec<Pending> {
        core::mem::take(&mut self.pending)
    }

    /// Here-docs a `$(…)` left without a body (`echo "$(cat <<B)"`) read theirs first,
    /// in order, before those of the line around it (bash).
    fn unstash_pending(&mut self, outer: Vec<Pending>) {
        let inner = core::mem::take(&mut self.pending);
        let (leaked, direct): (Vec<Pending>, Vec<Pending>) =
            outer.into_iter().partition(|p| p.leaked);
        self.pending = leaked;
        self.pending.extend(inner.into_iter().map(|mut p| {
            p.leaked = true;
            p
        }));
        self.pending.extend(direct);
    }

    /// At the end of the input, here-docs still waiting for a body get an empty one.
    fn finish_pending(&mut self) {
        for p in core::mem::take(&mut self.pending) {
            let doc = &mut self.heredocs[p.id.index()];
            doc.body = Word {
                span: sp(self.end, self.end),
                parts: Vec::new(),
            };
            doc.terminated = false;
        }
    }

    // ---- token peeking ----

    /// Skips line continuations (`\` newline) at `i`: bash removes them before
    /// tokenizing, so they may even split an operator.
    fn cont(&self, mut i: usize) -> usize {
        while i + 1 < self.end && self.b[i] == b'\\' && self.b[i + 1] == b'\n' {
            i += 2;
        }
        i
    }

    /// The logical chars at the cursor: up to three (char, index) pairs, skipping line
    /// continuations between them.
    fn chars3(&self, from: usize) -> [(Option<u8>, usize); 3] {
        let i0 = from;
        let i1 = self.cont(i0 + 1);
        let i2 = self.cont(i1 + 1);
        [
            (self.byte(i0), i0),
            (self.byte(i1), i1),
            (self.byte(i2), i2),
        ]
    }

    fn peek_op(&self) -> Option<(Op, usize)> {
        let [(c0, _), (c1, i1), (c2, i2)] = self.chars3(self.pos);
        let len = |n: usize| [1, i1 + 1 - self.pos, i2 + 1 - self.pos][n - 1];
        Some(match (c0?, c1) {
            (b'&', Some(b'&')) => (Op::AndIf, len(2)),
            (b'&', Some(b'>')) => return None,
            (b'&', _) => (Op::Amp, 1),
            (b'|', Some(b'|')) => (Op::OrIf, len(2)),
            (b'|', Some(b'&')) => (Op::PipeAmp, len(2)),
            (b'|', _) => (Op::Pipe, 1),
            (b';', Some(b';')) => {
                if c2 == Some(b'&') {
                    (Op::DSemiAnd, len(3))
                } else {
                    (Op::DSemi, len(2))
                }
            }
            (b';', Some(b'&')) => (Op::SemiAnd, len(2)),
            (b';', _) => (Op::Semi, 1),
            (b'(', _) => (Op::LParen, 1),
            (b')', _) => (Op::RParen, 1),
            _ => return None,
        })
    }

    /// The raw run of non-metachars at the cursor.
    fn run(&self) -> &'a [u8] {
        let mut i = self.pos;
        while i < self.end && !is_meta(self.b[i]) {
            // A line continuation followed by a token boundary ends the run too
            // (`if\⏎`), so a reserved word before it is still recognized.
            if self.b[i] == b'\\' && self.b.get(i + 1) == Some(&b'\n') {
                let j = self.cont(i);
                if j >= self.end || is_meta(self.b[j]) {
                    break;
                }
            }
            i += 1;
        }
        &self.b[self.pos..i]
    }

    fn peek_reserved(&self) -> Option<&'static str> {
        let run = self.run();
        if run.is_empty() || run.len() > 8 {
            return None;
        }
        let word = RESERVED.iter().copied().find(|r| r.as_bytes() == run)?;
        if word == "!" && self.opts.extglob && self.peek_at(1) == Some(b'(') {
            return None;
        }
        Some(word)
    }

    fn at_reserved(&self, word: &str) -> bool {
        self.peek_reserved() == Some(word)
    }

    fn expect_reserved(&mut self, word: &str) -> R<usize> {
        self.skip_blanks();
        if self.at_reserved(word) {
            let start = self.pos;
            self.pos += word.len();
            Ok(start)
        } else {
            Err(self.unexpected())
        }
    }

    /// A redirection at the cursor: (fd, operator, fd length, total operator length).
    fn peek_redirect(&self) -> Option<(Option<RedirFd>, RedirOp, usize, usize)> {
        let mut i = self.pos;
        let mut fd = None;
        let c = *self.b.get(i)?;
        if c.is_ascii_digit() {
            let mut j = i;
            let mut n: u32 = 0;
            while j < self.end && self.b[j].is_ascii_digit() {
                n = n
                    .saturating_mul(10)
                    .saturating_add((self.b[j] - b'0') as u32);
                j += 1;
            }
            // A number too large for a descriptor (bash: an int) is a word.
            if j < self.end && matches!(self.b[j], b'<' | b'>') && n <= i32::MAX as u32 {
                fd = Some(RedirFd::Number(n));
                i = j;
            } else {
                return None;
            }
        } else if c == b'{' {
            let mut j = i + 1;
            if j < self.end && is_name_start(self.b[j]) {
                while j < self.end && is_name_char(self.b[j]) {
                    j += 1;
                }
                if j + 1 < self.end && self.b[j] == b'}' && matches!(self.b[j + 1], b'<' | b'>') {
                    fd = Some(RedirFd::Var(sp(i + 1, j)));
                    i = j + 1;
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }
        let [(c0, _), (c1, i1), (c2, i2)] = self.chars3(i);
        let (op, len) = match (c0?, c1, c2) {
            (b'<', Some(b'<'), Some(b'<')) => (RedirOp::HereString, 3),
            (b'<', Some(b'<'), Some(b'-')) => (RedirOp::HereDocStrip, 3),
            (b'<', Some(b'<'), _) => (RedirOp::HereDoc, 2),
            (b'<', Some(b'&'), _) => (RedirOp::DupRead, 2),
            (b'<', Some(b'>'), _) => (RedirOp::ReadWrite, 2),
            (b'<', Some(b'('), _) if fd.is_none() => return None,
            (b'<', _, _) => (RedirOp::Read, 1),
            (b'>', Some(b'>'), _) => (RedirOp::Append, 2),
            (b'>', Some(b'&'), _) => (RedirOp::DupWrite, 2),
            (b'>', Some(b'|'), _) => (RedirOp::Clobber, 2),
            (b'>', Some(b'('), _) if fd.is_none() => return None,
            (b'>', _, _) => (RedirOp::Write, 1),
            (b'&', Some(b'>'), Some(b'>')) if fd.is_none() => (RedirOp::AppendBoth, 3),
            (b'&', Some(b'>'), _) if fd.is_none() => (RedirOp::WriteBoth, 2),
            _ => return None,
        };
        let end = [i + 1, i1 + 1, i2 + 1][len - 1];
        Some((fd, op, i - self.pos, end - self.pos))
    }

    fn starts_compound(&self) -> bool {
        if self.peek() == Some(b'(') {
            return true;
        }
        matches!(
            self.peek_reserved(),
            Some("{" | "if" | "while" | "until" | "for" | "select" | "case" | "[[")
        )
    }

    // ---- lists ----

    /// Parses one top-level line into `items`. Returns false at the end of input.
    fn parse_line(&mut self, items: &mut Vec<AndOr>) -> R<bool> {
        self.skip_linebreaks()?;
        self.maybe_alias();
        self.skip_linebreaks()?;
        if self.at_eof() {
            return Ok(false);
        }
        loop {
            let mut ao = self.parse_and_or()?;
            self.skip_blanks();
            match self.peek_op() {
                Some((Op::Semi, _)) => self.pos += 1,
                Some((Op::Amp, _)) => {
                    self.pos += 1;
                    ao.background = true;
                }
                _ => match self.peek() {
                    None => {
                        items.push(ao);
                        return Ok(true);
                    }
                    Some(b'\n') => {
                        ao.newline = true;
                        items.push(ao);
                        self.newline()?;
                        return Ok(true);
                    }
                    _ => return Err(self.unexpected()),
                },
            }
            items.push(ao);
            // After `;` or `&`: an alias here may leave nothing but the line end.
            self.maybe_alias();
            match self.peek() {
                None => return Ok(true),
                Some(b'\n') => {
                    self.newline()?;
                    return Ok(true);
                }
                _ => {}
            }
        }
    }

    fn at_term(&self, term: Term) -> bool {
        match term {
            Term::Paren => self.peek() == Some(b')'),
            Term::FunSubClose => self.peek() == Some(b'}'),
            Term::Words(words) => self.peek_reserved().is_some_and(|w| words.contains(&w)),
            Term::Case => {
                matches!(
                    self.peek_op(),
                    Some((Op::DSemi | Op::SemiAnd | Op::DSemiAnd, _))
                ) || self.at_reserved("esac")
            }
            Term::End => self.at_eof(),
        }
    }

    fn parse_compound_list(&mut self, term: Term, opener: Opener, allow_empty: bool) -> R<ListId> {
        self.enter()?;
        let mut items: Vec<AndOr> = Vec::new();
        loop {
            self.skip_linebreaks()?;
            // Aliases expand before reserved words are seen (`alias e=fi`).
            self.maybe_alias();
            self.skip_linebreaks()?;
            if self.at_term(term) {
                break;
            }
            if self.at_eof() {
                return Err(self.eof_from(opener));
            }
            let mut ao = self.parse_and_or()?;
            self.skip_blanks();
            match self.peek_op() {
                Some((Op::Semi, _)) => self.pos += 1,
                Some((Op::Amp, _)) => {
                    self.pos += 1;
                    ao.background = true;
                }
                _ => match self.peek() {
                    None => {}
                    Some(b'\n') => ao.newline = true,
                    _ if self.at_term(term) => {}
                    _ => return Err(self.unexpected()),
                },
            }
            items.push(ao);
        }
        if items.is_empty() && !allow_empty {
            return Err(self.unexpected());
        }
        self.leave();
        let span = match (items.first(), items.last()) {
            (Some(f), Some(l)) => join(f.span, l.span),
            _ => sp(self.pos, self.pos),
        };
        Ok(self.push_list(List { span, items }))
    }

    fn push_list(&mut self, list: List) -> ListId {
        let id = ListId(self.lists.len() as u32);
        self.lists.push(list);
        id
    }

    fn push_cmd(&mut self, cmd: Command) -> CmdId {
        let id = CmdId(self.cmds.len() as u32);
        self.cmds.push(cmd);
        id
    }

    fn parse_and_or(&mut self) -> R<AndOr> {
        let first = self.parse_pipeline()?;
        let mut rest = Vec::new();
        loop {
            self.skip_blanks();
            let (op, n) = match self.peek_op() {
                Some((Op::AndIf, n)) => (AndOrOp::And, n),
                Some((Op::OrIf, n)) => (AndOrOp::Or, n),
                _ => break,
            };
            self.pos += n;
            self.skip_linebreaks()?;
            // An alias with empty text here leaves the place where newlines may be.
            self.maybe_alias();
            self.skip_linebreaks()?;
            rest.push((op, self.parse_pipeline()?));
        }
        let span = match rest.last() {
            Some((_, p)) => join(first.span, p.span),
            None => first.span,
        };
        Ok(AndOr {
            span,
            first,
            rest,
            background: false,
            newline: false,
        })
    }

    fn parse_pipeline(&mut self) -> R<Pipeline> {
        self.skip_blanks();
        let start = self.pos;
        let mut time = None;
        let mut bangs = 0;
        let mut prefix_end = start;
        let mut prefixed = false;
        loop {
            self.maybe_alias();
            match self.peek_reserved() {
                Some("time") if time.is_none() => {
                    let s = self.pos;
                    self.pos += 4;
                    let mut end = self.pos;
                    self.skip_blanks();
                    let posix = self.run() == b"-p";
                    if posix {
                        self.pos += 2;
                        end = self.pos;
                    }
                    time = Some(Time {
                        span: sp(s, end),
                        posix,
                    });
                    prefix_end = self.pos;
                    prefixed = true;
                }
                Some("!") => {
                    self.pos += 1;
                    bangs += 1;
                    prefix_end = self.pos;
                    prefixed = true;
                }
                _ => break,
            }
        }
        let mut stages = Vec::new();
        if prefixed {
            // A bare `time` or `!` is a complete pipeline.
            let at_end = match self.peek() {
                None | Some(b'\n') => true,
                _ => matches!(self.peek_op(), Some((Op::Semi | Op::Amp, _))),
            };
            if at_end {
                return Ok(Pipeline {
                    span: sp(start, prefix_end),
                    time,
                    bangs,
                    stages,
                });
            }
        }
        loop {
            let cmd = self.parse_command()?;
            stages.push(Stage {
                cmd,
                pipe_stderr: false,
            });
            self.skip_blanks();
            match self.peek_op() {
                Some((Op::Pipe, _)) => self.pos += 1,
                Some((Op::PipeAmp, n)) => {
                    self.pos += n;
                    stages.last_mut().unwrap().pipe_stderr = true;
                }
                _ => break,
            }
            self.skip_linebreaks()?;
            self.maybe_alias();
            self.skip_linebreaks()?;
        }
        let end = self.cmds[stages.last().unwrap().cmd.index()].span.end;
        Ok(Pipeline {
            span: Span::new(start as u32, end),
            time,
            bangs,
            stages,
        })
    }

    // ---- commands ----

    fn parse_command(&mut self) -> R<CmdId> {
        self.maybe_alias();
        self.skip_blanks();
        let start = self.pos;
        let kind = if self.at(b"((") {
            match self.parse_arith_command()? {
                Some(k) => k,
                None => self.parse_subshell()?,
            }
        } else if self.peek() == Some(b'(') {
            self.parse_subshell()?
        } else {
            match self.peek_reserved() {
                Some("{") => {
                    self.pos += 1;
                    let body = self.parse_compound_list(
                        Term::Words(&["}"]),
                        Opener {
                            what: "{",
                            pos: start,
                            matching: false,
                        },
                        false,
                    )?;
                    self.expect_reserved("}")?;
                    CommandKind::Brace(body)
                }
                Some("if") => self.parse_if(start)?,
                Some(w @ ("while" | "until")) => {
                    self.pos += w.len();
                    let opener = Opener {
                        what: w,
                        pos: start,
                        matching: false,
                    };
                    let cond = self.parse_compound_list(Term::Words(&["do"]), opener, false)?;
                    let body = self.parse_do_group(opener)?;
                    CommandKind::While(WhileCommand {
                        until: w == "until",
                        cond,
                        body,
                    })
                }
                Some(w @ ("for" | "select")) => self.parse_for(start, w)?,
                Some("case") => self.parse_case(start)?,
                Some("[[") => self.parse_cond_command()?,
                Some("function") => return self.parse_function_keyword(start),
                Some("coproc") => return self.parse_coproc(start),
                Some(
                    "then" | "else" | "elif" | "fi" | "do" | "done" | "esac" | "}" | "!" | "in"
                    | "]]",
                ) => {
                    return Err(self.unexpected());
                }
                _ => return self.parse_simple(start),
            }
        };
        // The compound command ends with the token just consumed.
        let mut end = self.pos;
        let mut redirects = Vec::new();
        loop {
            self.skip_blanks();
            if self.peek_redirect().is_none() {
                break;
            }
            let r = self.parse_redirect()?;
            end = r.span.end as usize;
            redirects.push(r);
        }
        Ok(self.push_cmd(Command {
            span: sp(start, end),
            kind,
            redirects,
        }))
    }

    fn parse_subshell(&mut self) -> R<CommandKind> {
        let start = self.pos;
        self.pos += 1;
        let body = self.parse_compound_list(
            Term::Paren,
            Opener {
                what: "(",
                pos: start,
                matching: false,
            },
            false,
        )?;
        self.skip_blanks();
        if self.peek() != Some(b')') {
            return Err(self.unexpected());
        }
        self.pos += 1;
        Ok(CommandKind::Subshell(body))
    }

    /// `(( … ))`, or `None` if the parentheses do not close with `))` (then it is a
    /// nested subshell).
    fn parse_arith_command(&mut self) -> R<Option<CommandKind>> {
        let Some(close) = self.scan_arith(self.pos + 2)? else {
            return Ok(None);
        };
        let word = self.parse_arith_text(self.pos + 2, close)?;
        self.pos = close + 2;
        Ok(Some(CommandKind::Arith(word)))
    }

    /// Finds the `))` closing an arithmetic expression starting at `from`: the index of
    /// its first `)`. `Ok(None)` if a `)` at depth zero is not followed by another one
    /// (then the parentheses are nested subshells), an error at the end of the input.
    fn scan_arith(&self, from: usize) -> R<Option<usize>> {
        let mut depth = 0usize;
        let mut i = from;
        while i < self.end {
            match self.b[i] {
                b'\\' => i += 1,
                b'\'' => {
                    i += 1;
                    while i < self.end && self.b[i] != b'\'' {
                        i += 1;
                    }
                }
                b'"' => {
                    i += 1;
                    while i < self.end && self.b[i] != b'"' {
                        if self.b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                b'`' => {
                    i += 1;
                    while i < self.end && self.b[i] != b'`' {
                        if self.b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                b'(' => depth += 1,
                b')' => {
                    if depth == 0 {
                        return Ok(if i + 1 < self.end && self.b[i + 1] == b')' {
                            Some(i)
                        } else {
                            None
                        });
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
        Err(self.unterminated(")"))
    }

    /// Parses `[from, to)` as arithmetic text.
    fn parse_arith_text(&mut self, from: usize, to: usize) -> R<Word> {
        let saved_end = self.end;
        self.end = to;
        self.pos = from;
        let mut parts = Vec::new();
        let r = self.parse_parts(Ctx::Arith { semi: false }, &mut parts);
        self.end = saved_end;
        r?;
        Ok(Word {
            span: sp(from, to),
            parts,
        })
    }

    fn parse_if(&mut self, start: usize) -> R<CommandKind> {
        let opener = Opener {
            what: "if",
            pos: start,
            matching: false,
        };
        self.pos += 2;
        let mut branches = Vec::new();
        let mut else_body = None;
        let mut branch_start = start;
        loop {
            let cond = self.parse_compound_list(Term::Words(&["then"]), opener, false)?;
            self.expect_reserved("then")?;
            let body =
                self.parse_compound_list(Term::Words(&["elif", "else", "fi"]), opener, false)?;
            branches.push(IfBranch {
                span: sp(branch_start, self.pos),
                cond,
                body,
            });
            self.skip_blanks();
            match self.peek_reserved() {
                Some("elif") => {
                    branch_start = self.pos;
                    self.pos += 4;
                }
                Some("else") => {
                    self.pos += 4;
                    else_body =
                        Some(self.parse_compound_list(Term::Words(&["fi"]), opener, false)?);
                    self.expect_reserved("fi")?;
                    break;
                }
                _ => {
                    self.expect_reserved("fi")?;
                    break;
                }
            }
        }
        Ok(CommandKind::If(IfCommand {
            branches,
            else_body,
        }))
    }

    fn parse_do_group(&mut self, opener: Opener) -> R<ListId> {
        self.skip_blanks();
        if self.at_eof() {
            return Err(self.eof_from(opener));
        }
        self.expect_reserved("do")?;
        let body = self.parse_compound_list(Term::Words(&["done"]), opener, false)?;
        self.expect_reserved("done")?;
        Ok(body)
    }

    /// `do … done`, or `{ … }` (bash accepts it for `for` and `select`).
    fn parse_loop_body(&mut self, opener: Opener) -> R<ListId> {
        self.skip_blanks();
        if self.at_reserved("{") {
            let start = self.pos;
            self.pos += 1;
            let body = self.parse_compound_list(
                Term::Words(&["}"]),
                Opener {
                    what: "{",
                    pos: start,
                    matching: false,
                },
                false,
            )?;
            self.expect_reserved("}")?;
            return Ok(body);
        }
        self.parse_do_group(opener)
    }

    fn parse_for(&mut self, start: usize, kw: &'static str) -> R<CommandKind> {
        let opener = Opener {
            what: kw,
            pos: start,
            matching: false,
        };
        self.pos += kw.len();
        self.skip_blanks();
        if kw == "for" && self.at(b"((") {
            return self.parse_arith_for(opener);
        }
        let Some(var) = self.read_word(Ctx::Word, false)? else {
            return Err(self.unexpected());
        };
        self.skip_linebreaks()?;
        self.alias_after_blank();
        let mut words = None;
        if self.at_reserved("in") {
            self.pos += 2;
            let mut list = Vec::new();
            loop {
                self.skip_blanks();
                match self.read_word(Ctx::Word, true)? {
                    Some(w) => list.push(w),
                    None => break,
                }
            }
            words = Some(list);
            self.skip_blanks();
            match self.peek() {
                Some(b';') if self.peek_op() == Some((Op::Semi, 1)) => self.pos += 1,
                Some(b'\n') => {}
                None => return Err(self.eof_from(opener)),
                _ => return Err(self.unexpected()),
            }
            self.skip_linebreaks()?;
        } else if self.peek_op() == Some((Op::Semi, 1)) {
            self.pos += 1;
            self.skip_linebreaks()?;
        }
        let body = self.parse_loop_body(opener)?;
        let f = ForCommand { var, words, body };
        Ok(if kw == "for" {
            CommandKind::For(f)
        } else {
            CommandKind::Select(f)
        })
    }

    fn parse_arith_for(&mut self, opener: Opener) -> R<CommandKind> {
        let open = self.pos;
        let Some(close) = self.scan_arith(open + 2)? else {
            return Err(self.unterminated(")"));
        };
        let saved_end = self.end;
        self.end = close;
        self.pos = open + 2;
        let mut clauses = Vec::new();
        let result = self.arith_for_clauses(close, &mut clauses);
        let extra_semi = self.peek() == Some(b';');
        self.end = saved_end;
        if !result? {
            let message = if extra_semi {
                "syntax error: `;' unexpected"
            } else {
                "syntax error: arithmetic expression required"
            };
            return Err(self.error_at(open, close + 2, String::from(message)));
        }
        self.pos = close + 2;
        self.skip_blanks();
        if self.peek_op() == Some((Op::Semi, 1)) {
            self.pos += 1;
        }
        self.skip_linebreaks()?;
        let body = self.parse_loop_body(opener)?;
        let step = clauses.pop().unwrap();
        let cond = clauses.pop().unwrap();
        let init = clauses.pop().unwrap();
        Ok(CommandKind::ArithFor(ArithForCommand {
            init,
            cond,
            step,
            body,
        }))
    }

    /// The three `;`-separated clauses of `for ((…))`; false if there are not three.
    fn arith_for_clauses(&mut self, close: usize, clauses: &mut Vec<Word>) -> R<bool> {
        for i in 0..3 {
            let s = self.pos;
            let mut parts = Vec::new();
            self.parse_parts(Ctx::Arith { semi: true }, &mut parts)?;
            clauses.push(Word {
                span: sp(s, self.pos),
                parts,
            });
            if i < 2 {
                if self.peek() != Some(b';') {
                    return Ok(false);
                }
                self.pos += 1;
            } else if self.pos != close {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn parse_case(&mut self, start: usize) -> R<CommandKind> {
        let opener = Opener {
            what: "case",
            pos: start,
            matching: false,
        };
        self.pos += 4;
        self.skip_blanks();
        let Some(word) = self.read_word(Ctx::Word, false)? else {
            return Err(self.unexpected());
        };
        self.skip_linebreaks()?;
        if self.at_eof() {
            return Err(self.eof_from(opener));
        }
        self.expect_reserved("in")?;
        let mut items = Vec::new();
        loop {
            self.skip_linebreaks()?;
            if self.at_eof() {
                return Err(self.eof_from(opener));
            }
            if self.at_reserved("esac") {
                self.pos += 4;
                break;
            }
            let item_start = self.pos;
            if self.peek() == Some(b'(') {
                self.pos += 1;
            }
            let mut patterns = Vec::new();
            loop {
                self.skip_blanks();
                match self.read_word(
                    Ctx::Pattern {
                        ext: self.opts.extglob,
                    },
                    false,
                )? {
                    Some(w) => patterns.push(w),
                    None if self.at_eof() => return Err(self.eof_from(opener)),
                    None => return Err(self.unexpected()),
                }
                self.skip_blanks();
                if self.peek_op() == Some((Op::Pipe, 1)) {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if self.peek() != Some(b')') {
                if self.at_eof() {
                    return Err(self.eof_from(opener));
                }
                return Err(self.unexpected());
            }
            self.pos += 1;
            let body = self.parse_compound_list(Term::Case, opener, true)?;
            let body = if self.lists[body.index()].items.is_empty() {
                self.lists.pop();
                None
            } else {
                Some(body)
            };
            self.skip_blanks();
            let terminator = match self.peek_op() {
                Some((Op::DSemi, n)) => {
                    self.pos += n;
                    CaseTerminator::Break
                }
                Some((Op::SemiAnd, n)) => {
                    self.pos += n;
                    CaseTerminator::FallThrough
                }
                Some((Op::DSemiAnd, n)) => {
                    self.pos += n;
                    CaseTerminator::Continue
                }
                _ => CaseTerminator::Break,
            };
            items.push(CaseItem {
                span: sp(item_start, self.pos),
                patterns,
                body,
                terminator,
            });
        }
        Ok(CommandKind::Case(CaseCommand { word, items }))
    }

    fn parse_function_keyword(&mut self, start: usize) -> R<CmdId> {
        self.pos += "function".len();
        self.skip_blanks();
        let Some(name) = self.read_word(Ctx::Word, false)? else {
            return Err(self.unexpected());
        };
        self.skip_blanks();
        if self.peek() == Some(b'(') {
            self.pos += 1;
            self.skip_blanks();
            if self.peek() != Some(b')') {
                return Err(self.unexpected());
            }
            self.pos += 1;
        }
        self.finish_function(start, name, true)
    }

    fn finish_function(&mut self, start: usize, name: Word, keyword: bool) -> R<CmdId> {
        self.skip_linebreaks()?;
        if !self.starts_compound() {
            return Err(self.unexpected());
        }
        let body = self.parse_command()?;
        let end = self.cmds[body.index()].span.end;
        Ok(self.push_cmd(Command {
            span: Span::new(start as u32, end),
            kind: CommandKind::Function(FunctionDef {
                name,
                keyword,
                body,
            }),
            redirects: Vec::new(),
        }))
    }

    fn parse_coproc(&mut self, start: usize) -> R<CmdId> {
        self.pos += "coproc".len();
        self.skip_blanks();
        let mut name = None;
        let body = if self.starts_compound() {
            self.parse_command()?
        } else {
            let cp = self.checkpoint();
            let word = self.read_word(Ctx::Word, false)?;
            self.skip_blanks();
            if word.is_some() && self.starts_compound() {
                name = word;
                self.parse_command()?
            } else {
                self.rewind(cp);
                let s = self.pos;
                self.parse_simple(s)?
            }
        };
        let end = self.cmds[body.index()].span.end;
        Ok(self.push_cmd(Command {
            span: Span::new(start as u32, end),
            kind: CommandKind::Coproc(Coproc { name, body }),
            redirects: Vec::new(),
        }))
    }

    fn parse_simple(&mut self, start: usize) -> R<CmdId> {
        let mut assigns = Vec::new();
        let mut words: Vec<Word> = Vec::new();
        let mut redirects = Vec::new();
        let mut decl = false;
        let mut end = start;
        loop {
            self.skip_blanks();
            let Some(c) = self.peek() else { break };
            if self.peek_redirect().is_some() {
                let r = self.parse_redirect()?;
                end = r.span.end as usize;
                redirects.push(r);
                continue;
            }
            if c == b'\n' || (is_meta(c) && !self.at(b"<(") && !self.at(b">(")) {
                if c == b'(' && words.len() == 1 && assigns.is_empty() && redirects.is_empty() {
                    // `name ( )`: a function definition.
                    self.pos += 1;
                    self.skip_blanks();
                    if self.peek() != Some(b')') {
                        return Err(self.unexpected());
                    }
                    self.pos += 1;
                    let name = words.pop().unwrap();
                    return self.finish_function(start, name, false);
                }
                if c == b'(' {
                    return Err(self.unexpected());
                }
                break;
            }
            // The command word, and the word after an alias text that ends in a
            // blank, may be aliases.
            if words.is_empty() || core::mem::take(&mut self.alias_next) {
                self.maybe_alias();
                self.skip_blanks();
                if self.peek().is_none_or(|c| {
                    c == b'\n' || (is_meta(c) && !self.at(b"<(") && !self.at(b">("))
                }) {
                    continue;
                }
            }
            if words.is_empty()
                && let Some(a) = self.try_assign()?
            {
                end = a.span.end as usize;
                assigns.push(a);
                continue;
            }
            let word = if decl {
                self.read_decl_arg()?
            } else {
                self.read_word(Ctx::Word, true)?
            };
            let Some(word) = word else {
                return Err(self.unexpected());
            };
            if words.is_empty() && word.parts.len() == 1 && matches!(word.parts[0].kind, Part::Lit)
            {
                decl = DECL_BUILTINS
                    .iter()
                    .any(|d| d.as_bytes() == &self.b[word.span.range()]);
            }
            end = word.span.end as usize;
            words.push(word);
        }
        if assigns.is_empty() && words.is_empty() && redirects.is_empty() {
            // (An alias with empty text is as if the word were not there.)
            return Err(self.unexpected());
        }
        Ok(self.push_cmd(Command {
            span: sp(start, end),
            kind: CommandKind::Simple(SimpleCommand { assigns, words }),
            redirects,
        }))
    }

    // ---- redirections ----

    fn parse_redirect(&mut self) -> R<Redirect> {
        let start = self.pos;
        let (fd, op, _, len) = self.peek_redirect().expect("caller checked");
        self.pos += len;
        self.skip_blanks();
        let target = match op {
            RedirOp::HereDoc | RedirOp::HereDocStrip => {
                let Some(delimiter) = self.read_word(Ctx::Word, false)? else {
                    return Err(self.missing_word());
                };
                let (delim, quoted) = self.heredoc_delimiter(&delimiter);
                let id = HereDocId(self.heredocs.len() as u32);
                let strip = op == RedirOp::HereDocStrip;
                self.heredocs.push(HereDoc {
                    delimiter,
                    quoted,
                    strip_tabs: strip,
                    body: Word {
                        span: Span::default(),
                        parts: Vec::new(),
                    },
                    terminated: false,
                    error: None,
                });
                self.pending.push(Pending {
                    id,
                    delim,
                    strip,
                    quoted,
                    leaked: false,
                });
                RedirTarget::HereDoc(id)
            }
            _ => {
                let brace = op != RedirOp::HereString;
                let Some(w) = self.read_word(Ctx::Word, brace)? else {
                    return Err(self.missing_word());
                };
                RedirTarget::Word(w)
            }
        };
        let end = match &target {
            RedirTarget::Word(w) => w.span.end as usize,
            RedirTarget::HereDoc(id) => self.heredocs[id.index()].delimiter.span.end as usize,
        };
        Ok(Redirect {
            span: sp(start, end),
            fd,
            op,
            target,
        })
    }

    /// The delimiter text after quote removal, and whether any of it was quoted.
    fn heredoc_delimiter(&self, w: &Word) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        let mut quoted = false;
        fn walk(src: &[u8], parts: &[WordPart], out: &mut Vec<u8>, quoted: &mut bool) {
            for p in parts {
                let text = &src[p.span.range()];
                match &p.kind {
                    Part::Lit => out.extend_from_slice(text),
                    Part::Escaped => {
                        *quoted = true;
                        out.extend_from_slice(&text[1..]);
                    }
                    Part::Single => {
                        *quoted = true;
                        out.extend_from_slice(&text[1..text.len() - 1]);
                    }
                    Part::AnsiC => {
                        *quoted = true;
                        out.extend(crate::words::decode_ansi_c(&text[2..text.len() - 1]));
                    }
                    Part::Double(inner) => {
                        *quoted = true;
                        walk(src, inner, out, quoted);
                    }
                    Part::Locale(inner) => {
                        *quoted = true;
                        walk(src, inner, out, quoted);
                    }
                    _ => out.extend_from_slice(text),
                }
            }
        }
        walk(self.b, &w.parts, &mut out, &mut quoted);
        (out, quoted)
    }

    // ---- assignments ----

    fn name_end(&self, from: usize) -> Option<usize> {
        if from < self.end && is_name_start(self.b[from]) {
            let mut i = from + 1;
            while i < self.end && is_name_char(self.b[i]) {
                i += 1;
            }
            Some(i)
        } else {
            None
        }
    }

    /// `name[sub]` followed by `=` or `+=` at the cursor: (name span, index, append),
    /// with the cursor after the `=`. Rewinds and returns `None` otherwise.
    fn assign_head(&mut self) -> R<Option<(Span, Option<Word>, bool)>> {
        let start = self.pos;
        let Some(name_end) = self.name_end(start) else {
            return Ok(None);
        };
        let cp = self.checkpoint();
        self.pos = name_end;
        let mut index = None;
        if self.peek() == Some(b'[') {
            self.pos += 1;
            let s = self.pos;
            let mut parts = Vec::new();
            self.parse_parts(Ctx::Subscript, &mut parts)?;
            if self.at_eof() {
                // Bash insists on the `]` of a subscript where an assignment may start.
                return Err(self.unterminated("]"));
            }
            if self.peek() != Some(b']') {
                self.rewind(cp);
                return Ok(None);
            }
            index = Some(Word {
                span: sp(s, self.pos),
                parts,
            });
            self.pos += 1;
        }
        let append = if self.at(b"+=") {
            self.pos += 2;
            true
        } else if self.at(b"=") {
            self.pos += 1;
            false
        } else {
            self.rewind(cp);
            return Ok(None);
        };
        Ok(Some((sp(start, name_end), index, append)))
    }

    fn try_assign(&mut self) -> R<Option<Assign>> {
        let start = self.pos;
        let Some((name, index, append)) = self.assign_head()? else {
            return Ok(None);
        };
        let value = if self.peek() == Some(b'(') {
            AssignValue::Array(self.parse_array_lit()?)
        } else {
            let s = self.pos;
            let mut parts = Vec::new();
            self.parse_parts(Ctx::Word, &mut parts)?;
            AssignValue::Scalar(Word {
                span: sp(s, self.pos),
                parts,
            })
        };
        Ok(Some(Assign {
            span: sp(start, self.pos),
            name,
            index,
            append,
            value,
        }))
    }

    /// An argument of `declare` and friends: `name=( … )` is an array; anything else is
    /// an ordinary word.
    fn read_decl_arg(&mut self) -> R<Option<Word>> {
        let start = self.pos;
        let cp = self.checkpoint();
        if self.assign_head()?.is_some() && self.peek() == Some(b'(') {
            let head_end = self.pos;
            let lit = self.parse_array_lit()?;
            let mut parts = Vec::new();
            push_lit(&mut parts, start, head_end);
            parts.push(WordPart {
                span: lit.span,
                kind: Part::Array(lit),
            });
            self.parse_parts(Ctx::Word, &mut parts)?;
            return Ok(Some(Word {
                span: sp(start, self.pos),
                parts,
            }));
        }
        self.rewind(cp);
        self.read_word(Ctx::Word, true)
    }

    fn parse_array_lit(&mut self) -> R<ArrayLit> {
        let start = self.pos;
        self.pos += 1;
        let mut elems = Vec::new();
        loop {
            self.skip_linebreaks()?;
            match self.peek() {
                None => return Err(self.unterminated(")")),
                Some(b')') => {
                    self.pos += 1;
                    break;
                }
                _ => {}
            }
            let es = self.pos;
            let mut key = None;
            let mut append = false;
            if self.peek() == Some(b'[') {
                let cp = self.checkpoint();
                self.pos += 1;
                let ks = self.pos;
                let mut parts = Vec::new();
                self.parse_parts(Ctx::Subscript, &mut parts)?;
                let ke = self.pos;
                if self.peek() == Some(b']') && (self.at(b"]=") || self.at(b"]+=")) {
                    append = self.at(b"]+=");
                    self.pos += if append { 3 } else { 2 };
                    key = Some(Word {
                        span: sp(ks, ke),
                        parts,
                    });
                } else {
                    self.rewind(cp);
                }
            }
            let value = if key.is_some() {
                let s = self.pos;
                let mut parts = Vec::new();
                self.parse_parts(Ctx::Word, &mut parts)?;
                let parts = brace_pass(self.b, parts);
                Word {
                    span: sp(s, self.pos),
                    parts,
                }
            } else {
                match self.read_word(Ctx::Word, true)? {
                    Some(w) => w,
                    None => return Err(self.unexpected()),
                }
            };
            elems.push(ArrayElem {
                span: sp(es, self.pos),
                key,
                append,
                value,
            });
        }
        Ok(ArrayLit {
            span: sp(start, self.pos),
            elems,
        })
    }

    // ---- [[ ]] ----

    fn parse_cond_command(&mut self) -> R<CommandKind> {
        let open = self.pos;
        self.pos += 2;
        let expr = self.parse_cond_or(open)?;
        self.skip_linebreaks()?;
        if !self.at_reserved("]]") {
            if self.at_eof() {
                return Err(self.eof_error(format!(
                    "unexpected EOF while looking for `]]'\nsyntax error: unexpected end of file from `[[' command on line {}",
                    self.line_of(open)
                )));
            }
            let (s, e, text) = self.cond_token();
            return Err(self.error_at(
                s,
                e,
                format!(
                    "syntax error in conditional expression: unexpected token `{text}'\nsyntax error near `{text}'"
                ),
            ));
        }
        self.pos += 2;
        Ok(CommandKind::Cond(expr))
    }

    /// The token at the cursor inside `[[ ]]`, as bash names it in messages.
    fn cond_token(&self) -> (usize, usize, String) {
        let s = self.pos;
        match self.peek() {
            None => (s, s, String::from("EOF")),
            Some(b'\n') => (s, s + 1, String::from("newline")),
            Some(_) => {
                let two = [
                    &b"&&"[..],
                    b"||",
                    b"<>",
                    b">>",
                    b"<<",
                    b";;",
                    b"&>",
                    b">|",
                    b"<&",
                    b">&",
                ];
                let e = if two.iter().any(|t| self.at(t)) {
                    s + 2
                } else {
                    let run = self.run().len();
                    s + run.max(1)
                };
                (s, e, self.lossy(s, e))
            }
        }
    }

    /// The token bash names after "syntax error near" for a `[[ ]]` token: of some
    /// two-char operators only the last char.
    fn cond_near(text: &str) -> &str {
        match text {
            ";;" => ";",
            ">|" => "|",
            "<&" | ">&" => "&",
            t => t,
        }
    }

    /// A primary was expected at the cursor.
    fn cond_primary_error(&self, open: usize) -> ParseError {
        let (s, e, text) = self.cond_token();
        if self.at_eof() {
            return self.eof_error(format!(
                "unexpected token `EOF' in conditional command\nsyntax error: unexpected end of file from `[[' command on line {}",
                self.line_of(open)
            ));
        }
        let message = if text == "]]" {
            String::from("syntax error near `]]'")
        } else {
            format!(
                "unexpected token `{text}' in conditional command\nsyntax error near `{}'",
                Self::cond_near(&text)
            )
        };
        self.error_at(s, e, message)
    }

    /// The argument of a unary or binary operator is missing.
    fn cond_arg_error(&self, kind: &str, op: &str) -> ParseError {
        let (s, e, text) = self.cond_token();
        let near = if text == "newline" || text == "EOF" {
            op
        } else {
            Self::cond_near(&text)
        };
        self.error_at(
            s,
            e,
            format!(
                "unexpected argument `{text}' to conditional {kind} operator\nsyntax error near `{near}'"
            ),
        )
    }

    fn parse_cond_or(&mut self, open: usize) -> R<CondExpr> {
        let mut lhs = self.parse_cond_and(open)?;
        // Newlines may separate the operands and operators of `[[ ]]`, except between a
        // word and its binary operator.
        loop {
            self.skip_linebreaks()?;
            if !self.at(b"||") {
                return Ok(lhs);
            }
            self.pos += 2;
            self.skip_linebreaks()?;
            let rhs = self.parse_cond_and(open)?;
            lhs = CondExpr {
                span: join(lhs.span, rhs.span),
                kind: CondKind::Or(Box::new(lhs), Box::new(rhs)),
            };
        }
    }

    fn parse_cond_and(&mut self, open: usize) -> R<CondExpr> {
        let mut lhs = self.parse_cond_not(open)?;
        loop {
            self.skip_linebreaks()?;
            if !self.at(b"&&") {
                return Ok(lhs);
            }
            self.pos += 2;
            self.skip_linebreaks()?;
            let rhs = self.parse_cond_not(open)?;
            lhs = CondExpr {
                span: join(lhs.span, rhs.span),
                kind: CondKind::And(Box::new(lhs), Box::new(rhs)),
            };
        }
    }

    fn parse_cond_not(&mut self, open: usize) -> R<CondExpr> {
        self.skip_linebreaks()?;
        if self.run() == b"!" {
            let start = self.pos;
            self.pos += 1;
            let inner = self.parse_cond_not(open)?;
            return Ok(CondExpr {
                span: sp(start, inner.span.end as usize),
                kind: CondKind::Not(Box::new(inner)),
            });
        }
        self.parse_cond_primary(open)
    }

    /// The cursor is at a token that can be an operand word.
    fn at_cond_word(&self) -> bool {
        !self.at_eof() && self.run() != b"]]" && self.peek().is_some_and(|c| !is_meta(c))
            || self.at(b"<(")
            || self.at(b">(")
    }

    fn parse_cond_primary(&mut self, open: usize) -> R<CondExpr> {
        self.enter()?;
        self.skip_linebreaks()?;
        let start = self.pos;
        if self.peek() == Some(b'(') {
            self.pos += 1;
            let inner = self.parse_cond_or(open)?;
            self.skip_linebreaks()?;
            if self.peek() != Some(b')') {
                let (s, e, text) = self.cond_token();
                return Err(self.error_at(
                    s,
                    e,
                    format!("unexpected token `{text}', expected `)'\nsyntax error near `{text}'"),
                ));
            }
            self.pos += 1;
            self.leave();
            return Ok(CondExpr {
                span: sp(start, self.pos),
                kind: CondKind::Group(Box::new(inner)),
            });
        }
        let plain = Ctx::Pattern { ext: false };
        if !self.at_cond_word() {
            return Err(self.cond_primary_error(open));
        }
        let Some(first) = self.read_word(plain, false)? else {
            return Err(self.cond_primary_error(open));
        };
        let first_text = &*self.lossy(first.span.start as usize, first.span.end as usize);
        let first_plain = first.parts.len() == 1 && matches!(first.parts[0].kind, Part::Lit);
        if first_plain && let Some(op) = UnaryTest::from_op(first_text) {
            self.skip_blanks();
            if !self.at_cond_word() {
                return Err(self.cond_arg_error("unary", first_text));
            }
            let Some(arg) = self.read_word(plain, false)? else {
                return Err(self.cond_arg_error("unary", first_text));
            };
            self.leave();
            return Ok(CondExpr {
                span: sp(start, arg.span.end as usize),
                kind: CondKind::Unary {
                    op,
                    op_span: first.span,
                    arg,
                },
            });
        }
        self.skip_blanks();
        let op_start = self.pos;
        let op = match self.peek() {
            Some(b'<') if !self.at(b"<(") => {
                self.pos += 1;
                Some(BinaryTest::LeftSortsBeforeRight)
            }
            Some(b'>') if !self.at(b">(") => {
                self.pos += 1;
                Some(BinaryTest::LeftSortsAfterRight)
            }
            _ => {
                let run = self.run();
                match core::str::from_utf8(run).ok().and_then(BinaryTest::from_op) {
                    Some(op) => {
                        self.pos += run.len();
                        Some(op)
                    }
                    None => None,
                }
            }
        };
        let Some(op) = op else {
            let ends = self.at_eof()
                || self.run() == b"]]"
                || self.at(b"&&")
                || self.at(b"||")
                || self.peek() == Some(b')');
            if ends {
                self.leave();
                return Ok(CondExpr {
                    span: first.span,
                    kind: CondKind::Word(first),
                });
            }
            let (s, e, text) = self.cond_token();
            return Err(self.error_at(
                s,
                e,
                format!(
                    "unexpected token `{text}', conditional binary operator expected\nsyntax error near `{}'",
                    if text == "newline" { first_text } else { &text }
                ),
            ));
        };
        let op_span = sp(op_start, self.pos);
        let op_text = &*self.lossy(op_span.start as usize, op_span.end as usize);
        self.skip_blanks();
        let rhs_ctx = match op {
            BinaryTest::StringMatchesRegex => Ctx::Regex,
            BinaryTest::StringExactlyMatchesPattern
            | BinaryTest::StringDoesNotExactlyMatchPattern => Ctx::Pattern { ext: true },
            _ => plain,
        };
        // The right side of `=~` may start with `(` or `|`; other operands may not. For
        // `=~`, an operator there is a syntax error in the expression (bash).
        let regex = op == BinaryTest::StringMatchesRegex;
        let rhs_ok = self.at_cond_word() || (regex && matches!(self.peek(), Some(b'(' | b'|')));
        if !rhs_ok {
            let (s, e, text) = self.cond_token();
            if regex && !matches!(&*text, "]]" | "newline" | "EOF") {
                return Err(self.error_at(
                    s,
                    e,
                    format!(
                        "syntax error in conditional expression: unexpected token `{text}'\nsyntax error near `{}'",
                        Self::cond_near(&text)
                    ),
                ));
            }
            return Err(self.cond_arg_error("binary", op_text));
        }
        let Some(rhs) = self.read_word(rhs_ctx, false)? else {
            return Err(self.cond_arg_error("binary", op_text));
        };
        self.leave();
        Ok(CondExpr {
            span: join(first.span, rhs.span),
            kind: CondKind::Binary {
                op,
                op_span,
                lhs: first,
                rhs,
            },
        })
    }

    // ---- words ----

    fn read_word(&mut self, ctx: Ctx, brace: bool) -> R<Option<Word>> {
        self.alias_after_blank();
        let start = self.pos;
        let mut parts = Vec::new();
        self.parse_parts(ctx, &mut parts)?;
        if self.pos == start {
            return Ok(None);
        }
        let parts = if brace {
            brace_pass(self.b, parts)
        } else {
            parts
        };
        Ok(Some(Word {
            span: sp(start, self.pos),
            parts,
        }))
    }

    /// Reads word parts in `ctx` until its terminator (not consumed) or the end.
    fn parse_parts(&mut self, ctx: Ctx, out: &mut Vec<WordPart>) -> R<()> {
        self.enter()?;
        let r = self.parse_parts_inner(ctx, out);
        self.leave();
        r
    }

    fn parse_parts_inner(&mut self, ctx: Ctx, out: &mut Vec<WordPart>) -> R<()> {
        let mut depth = 0u32;
        let dq = ctx.dq();
        while let Some(c) = self.peek() {
            let start = self.pos;
            match c {
                b'\\' => match self.peek_at(1) {
                    Some(b'\n') => self.pos += 2,
                    None => {
                        self.pos += 1;
                        push_lit(out, start, self.pos);
                    }
                    Some(n) => {
                        // In a `${…}` operand a backslash always takes the next char
                        // along (inside double quotes the engine keeps the backslash
                        // where bash does).
                        let escapable = !dq
                            || matches!(n, b'$' | b'`' | b'\\')
                            || (n == b'"' && !matches!(ctx, Ctx::HereDoc { .. }))
                            || matches!(ctx, Ctx::Operand { .. });
                        if escapable {
                            self.pos += 1 + char_len(self.b, self.pos + 1, self.end);
                            out.push(WordPart {
                                span: sp(start, self.pos),
                                kind: Part::Escaped,
                            });
                        } else {
                            self.pos += 1;
                            push_lit(out, start, self.pos);
                        }
                    }
                },
                b'\'' => {
                    if matches!(ctx, Ctx::Double | Ctx::HereDoc { .. }) {
                        self.pos += 1;
                        push_lit(out, start, self.pos);
                    } else {
                        self.pos += 1;
                        while self.peek().is_some_and(|c| c != b'\'') {
                            self.pos += 1;
                        }
                        if self.at_eof() {
                            return Err(self.unterminated("'"));
                        }
                        self.pos += 1;
                        out.push(self.single_quoted(start, self.pos));
                    }
                }
                b'"' => match ctx {
                    Ctx::Double => return Ok(()),
                    Ctx::HereDoc { .. } => {
                        self.pos += 1;
                        push_lit(out, start, self.pos);
                    }
                    _ => {
                        self.pos += 1;
                        let mut inner = Vec::new();
                        self.parse_parts(Ctx::Double, &mut inner)?;
                        if self.peek() != Some(b'"') {
                            return Err(self.unterminated("\""));
                        }
                        self.pos += 1;
                        out.push(WordPart {
                            span: sp(start, self.pos),
                            kind: Part::Double(inner),
                        });
                    }
                },
                b'`' => {
                    self.pos += 1;
                    loop {
                        match self.peek() {
                            None => return Err(self.unterminated("`")),
                            Some(b'\\') => self.pos += 2.min(self.end - self.pos),
                            Some(b'`') => break,
                            Some(_) => self.pos += 1,
                        }
                    }
                    self.pos += 1;
                    out.push(WordPart {
                        span: sp(start, self.pos),
                        kind: Part::Backtick,
                    });
                }
                b'$' => self.parse_dollar(ctx, out)?,
                b'<' | b'>'
                    if self.peek_at(1) == Some(b'(')
                        && matches!(ctx, Ctx::Word | Ctx::Pattern { .. }) =>
                {
                    self.pos += 2;
                    let list = self.parse_nested_list(start, ")")?;
                    out.push(WordPart {
                        span: sp(start, self.pos),
                        kind: Part::ProcSubst {
                            output: c == b'>',
                            list,
                        },
                    });
                }
                b'?' | b'*' | b'+' | b'@' | b'!'
                    if self.peek_at(1) == Some(b'(') && self.extglob_in(ctx) =>
                {
                    self.parse_extglob(out)?;
                }
                _ => {
                    // Terminators and depth tracking per context.
                    let stop = match ctx {
                        Ctx::Word | Ctx::Pattern { .. } => is_meta(c),
                        Ctx::Regex => match c {
                            b'(' => {
                                depth += 1;
                                false
                            }
                            b')' if depth > 0 => {
                                depth -= 1;
                                false
                            }
                            b')' => true,
                            b' ' | b'\t' | b'\n' | b';' | b'&' | b'<' | b'>' => depth == 0,
                            _ => false,
                        },
                        Ctx::ExtGlob => match c {
                            b'(' => {
                                depth += 1;
                                false
                            }
                            b')' | b'|' if depth == 0 => true,
                            b')' => {
                                depth -= 1;
                                false
                            }
                            _ => false,
                        },
                        // Bash does not pair braces in `${…}`: the first unquoted
                        // `}` ends it (`${x:-{a}b}` is `${x:-{a}` and `b}`).
                        Ctx::Operand { stop, .. } => c == b'}' || (stop != 0 && c == stop),
                        Ctx::Subscript | Ctx::ArithBracket => match c {
                            b'[' => {
                                depth += 1;
                                false
                            }
                            b']' if depth == 0 => true,
                            b']' => {
                                depth -= 1;
                                false
                            }
                            _ => false,
                        },
                        Ctx::Arith { semi } => match c {
                            b'(' => {
                                depth += 1;
                                false
                            }
                            b')' if depth > 0 => {
                                depth -= 1;
                                false
                            }
                            b')' => true,
                            b';' => semi && depth == 0,
                            _ => false,
                        },
                        Ctx::Double => false,
                        Ctx::HereDoc { strip } => {
                            if c == b'\n' && strip {
                                self.pos += 1;
                                push_lit(out, start, self.pos);
                                while self.peek() == Some(b'\t') {
                                    self.pos += 1;
                                }
                                continue;
                            }
                            false
                        }
                    };
                    if stop {
                        return Ok(());
                    }
                    self.pos += char_len(self.b, self.pos, self.end);
                    push_lit(out, start, self.pos);
                }
            }
        }
        Ok(())
    }

    /// `'…'` from `start` to `end`. In a here-doc body its line continuations are gone
    /// (bash removes them first): then it is the same text as literal pieces in
    /// double quotes, which keeps the spans on the script.
    fn single_quoted(&self, start: usize, end: usize) -> WordPart {
        let inner = &self.b[start + 1..end - 1];
        if self.heredoc_body == 0 || !inner.windows(2).any(|w| w == b"\\\n") {
            return WordPart {
                span: sp(start, end),
                kind: Part::Single,
            };
        }
        let mut pieces = Vec::new();
        let mut s = start + 1;
        let mut i = s;
        while i < end - 1 {
            if self.b[i] == b'\\' && self.b.get(i + 1) == Some(&b'\n') && i + 1 < end - 1 {
                if i > s {
                    push_lit(&mut pieces, s, i);
                }
                i += 2;
                s = i;
            } else {
                i += 1;
            }
        }
        if end - 1 > s {
            push_lit(&mut pieces, s, end - 1);
        }
        WordPart {
            span: sp(start, end),
            kind: Part::Double(pieces),
        }
    }

    fn extglob_in(&self, ctx: Ctx) -> bool {
        match ctx {
            Ctx::Word => self.opts.extglob,
            Ctx::Pattern { ext } => ext || self.opts.extglob,
            Ctx::ExtGlob => true,
            _ => false,
        }
    }

    fn parse_extglob(&mut self, out: &mut Vec<WordPart>) -> R<()> {
        let start = self.pos;
        let op = self.b[start];
        self.pos += 2;
        let mut alts = Vec::new();
        loop {
            let s = self.pos;
            let mut parts = Vec::new();
            self.parse_parts(Ctx::ExtGlob, &mut parts)?;
            alts.push(Word {
                span: sp(s, self.pos),
                parts,
            });
            match self.peek() {
                Some(b'|') => self.pos += 1,
                Some(b')') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.unterminated(")")),
            }
        }
        out.push(WordPart {
            span: sp(start, self.pos),
            kind: Part::ExtGlob { op, alts },
        });
        Ok(())
    }

    /// Parses the list of `$(…)` or `<(…)` up to and including the `)`.
    fn parse_nested_list(&mut self, start: usize, what: &'static str) -> R<ListId> {
        self.comsub += 1;
        let outer = self.stash_pending();
        let closer = self.closer.replace(")");
        let r = self.parse_compound_list(
            Term::Paren,
            Opener {
                what,
                pos: start,
                matching: true,
            },
            true,
        );
        self.unstash_pending(outer);
        self.comsub -= 1;
        self.closer = closer;
        let list = r?;
        self.skip_blanks();
        if self.peek() != Some(b')') {
            return Err(self.unexpected());
        }
        self.pos += 1;
        Ok(list)
    }

    /// The `)` that balances an open `(` before `from`, counting parens outside quotes
    /// and backslash escapes.
    fn paren_end(&self, from: usize) -> Option<usize> {
        let mut depth = 1;
        let mut i = from;
        while i < self.end {
            match self.b[i] {
                b'\\' => i += 1,
                b'\'' => {
                    i += 1;
                    while i < self.end && self.b[i] != b'\'' {
                        i += 1;
                    }
                }
                b'"' => {
                    i += 1;
                    while i < self.end && self.b[i] != b'"' {
                        if self.b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// The list of a `$(` whose `)` is at `close`: parsed as if the text ended there.
    fn parse_bounded_list(&mut self, start: usize, close: usize) -> R<ListId> {
        let end = core::mem::replace(&mut self.end, close);
        self.comsub += 1;
        let outer = self.stash_pending();
        let r = self.parse_compound_list(
            Term::End,
            Opener {
                what: ")",
                pos: start,
                matching: true,
            },
            true,
        );
        self.unstash_pending(outer);
        self.comsub -= 1;
        self.end = end;
        let list = r?;
        if self.pos != close {
            return Err(self.unexpected());
        }
        self.pos = close + 1;
        Ok(list)
    }

    fn parse_dollar(&mut self, ctx: Ctx, out: &mut Vec<WordPart>) -> R<()> {
        let start = self.pos;
        let next = self.cont(start + 1);
        let special = |c: u8| {
            matches!(c, b'(' | b'{' | b'[' | b'\'' | b'"') || is_name_char(c) || is_special_param(c)
        };
        if next == start + 1 || !self.byte(next).is_some_and(special) {
            return self.parse_dollar_here(ctx, out);
        }
        // `$` and what follows are split by a line continuation: parse as if the `$`
        // were right before the next char, then let the part cover both.
        self.pos = next - 1;
        let before = out.len();
        self.parse_dollar_here(ctx, out)?;
        if out.len() > before {
            out.last_mut().unwrap().span.start = start as u32;
        }
        Ok(())
    }

    fn parse_dollar_here(&mut self, ctx: Ctx, out: &mut Vec<WordPart>) -> R<()> {
        let start = self.pos;
        let dq = ctx.dq();
        match self.peek_at(1) {
            Some(b'(') => {
                if self.peek_at(2) == Some(b'(')
                    && let Some(close) = self.scan_arith(start + 3)?
                {
                    let word = self.parse_arith_text(start + 3, close)?;
                    self.pos = close + 2;
                    out.push(WordPart {
                        span: sp(start, self.pos),
                        kind: Part::Arith(Box::new(word)),
                    });
                    return Ok(());
                }
                self.pos += 2;
                let list = match ctx {
                    // In an extglob, bash finds the end of `$(` by counting parens (a
                    // `)` in a comment counts) and parses the text inside only then.
                    Ctx::ExtGlob => match self.paren_end(self.pos) {
                        Some(close) => self.parse_bounded_list(start, close)?,
                        None => self.parse_nested_list(start, ")")?,
                    },
                    _ => self.parse_nested_list(start, ")")?,
                };
                out.push(WordPart {
                    span: sp(start, self.pos),
                    kind: Part::CmdSubst(list),
                });
            }
            Some(b'[') => {
                self.pos += 2;
                let s = self.pos;
                let mut parts = Vec::new();
                self.parse_parts(Ctx::ArithBracket, &mut parts)?;
                if self.peek() != Some(b']') {
                    return Err(self.unterminated("]"));
                }
                let word = Word {
                    span: sp(s, self.pos),
                    parts,
                };
                self.pos += 1;
                out.push(WordPart {
                    span: sp(start, self.pos),
                    kind: Part::Arith(Box::new(word)),
                });
            }
            Some(b'{') => {
                let part = self.parse_braced_param(dq)?;
                out.push(part);
            }
            // `$'…'` also in a double-quoted `${…}` operand (bash's extquote, on by
            // default).
            Some(b'\'') if !dq || matches!(ctx, Ctx::Operand { dq: true, .. }) => {
                self.pos += 2;
                loop {
                    match self.peek() {
                        None => return Err(self.unterminated("'")),
                        Some(b'\\') => self.pos += 2.min(self.end - self.pos),
                        Some(b'\'') => break,
                        Some(_) => self.pos += 1,
                    }
                }
                self.pos += 1;
                out.push(WordPart {
                    span: sp(start, self.pos),
                    kind: Part::AnsiC,
                });
            }
            Some(b'"') if !dq => {
                self.pos += 2;
                let mut inner = Vec::new();
                self.parse_parts(Ctx::Double, &mut inner)?;
                if self.peek() != Some(b'"') {
                    return Err(self.unterminated("\""));
                }
                self.pos += 1;
                out.push(WordPart {
                    span: sp(start, self.pos),
                    kind: Part::Locale(inner),
                });
            }
            Some(c) if is_name_start(c) => {
                let end = self.name_end(start + 1).unwrap();
                self.pos = end;
                out.push(self.simple_param(start, ParamName::Var, sp(start + 1, end)));
            }
            Some(c) if c.is_ascii_digit() => {
                self.pos = start + 2;
                out.push(self.simple_param(start, ParamName::Positional, sp(start + 1, start + 2)));
            }
            Some(c) if is_special_param(c) => {
                self.pos = start + 2;
                out.push(self.simple_param(start, ParamName::Special(c), sp(start + 1, start + 2)));
            }
            _ => {
                self.pos += 1;
                push_lit(out, start, self.pos);
            }
        }
        Ok(())
    }

    fn simple_param(&self, start: usize, name: ParamName, name_span: Span) -> WordPart {
        WordPart {
            span: sp(start, self.pos),
            kind: Part::Param(Box::new(Param {
                braced: false,
                prefix: ParamPrefix::None,
                name,
                name_span,
                index: None,
                op: ParamOp::None,
            })),
        }
    }

    /// A parameter name at `i`: its kind and end.
    fn param_name_at(&self, i: usize) -> Option<(ParamName, usize)> {
        let c = *self.b.get(i).filter(|_| i < self.end)?;
        if is_name_start(c) {
            Some((ParamName::Var, self.name_end(i).unwrap()))
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < self.end && self.b[j].is_ascii_digit() {
                j += 1;
            }
            Some((ParamName::Positional, j))
        } else if is_special_param(c) {
            Some((ParamName::Special(c), i + 1))
        } else {
            None
        }
    }

    fn byte(&self, i: usize) -> Option<u8> {
        if i < self.end { Some(self.b[i]) } else { None }
    }

    fn parse_braced_param(&mut self, dq: bool) -> R<WordPart> {
        let start = self.pos;
        let p = start + 2;
        if let Some(c @ (b' ' | b'\t' | b'\n' | b'|')) = self.byte(p) {
            // `${ list; }` and `${| list; }`: bash 5.3's command substitution in the
            // current shell.
            self.pos = if c == b'|' { p + 1 } else { p };
            let outer = self.stash_pending();
            let closer = self.closer.replace("}");
            let r = self.parse_compound_list(
                Term::FunSubClose,
                Opener {
                    what: "}",
                    pos: start,
                    matching: true,
                },
                true,
            );
            self.unstash_pending(outer);
            self.closer = closer;
            let list = r?;
            self.skip_blanks();
            if self.peek() != Some(b'}') {
                return Err(self.unexpected());
            }
            self.pos += 1;
            return Ok(WordPart {
                span: sp(start, self.pos),
                kind: Part::FunSub {
                    list,
                    reply: c == b'|',
                },
            });
        }
        let mut prefix = ParamPrefix::None;
        let head = match self.byte(p) {
            Some(b'#') => match self.byte(p + 1) {
                Some(b'}') => Some((ParamName::Special(b'#'), p, p + 1)),
                _ => match self.param_name_at(p + 1) {
                    Some((n, e)) if matches!(self.byte(e), Some(b'}' | b'[')) => {
                        prefix = ParamPrefix::Length;
                        Some((n, p + 1, e))
                    }
                    _ => Some((ParamName::Special(b'#'), p, p + 1)),
                },
            },
            Some(b'!') => match self.byte(p + 1) {
                Some(b'}') => Some((ParamName::Special(b'!'), p, p + 1)),
                _ => match self.param_name_at(p + 1) {
                    Some((n, e)) => {
                        prefix = ParamPrefix::Indirect;
                        Some((n, p + 1, e))
                    }
                    None => Some((ParamName::Special(b'!'), p, p + 1)),
                },
            },
            // `${$` followed by `{` or `(` nests an expansion (`${${x}}`): not the
            // pid, and not valid; scanned like any bad substitution.
            Some(b'$') if matches!(self.byte(p + 1), Some(b'{' | b'(')) => None,
            _ => self.param_name_at(p).map(|(n, e)| (n, p, e)),
        };
        let Some((name, ns, ne)) = head else {
            self.pos = p;
            return self.finish_bad_subst(start, dq);
        };
        self.pos = ne;
        let mut index = None;
        if name == ParamName::Var && self.peek() == Some(b'[') {
            if self.at(b"[@]") {
                index = Some(Subscript::At);
                self.pos += 3;
            } else if self.at(b"[*]") {
                index = Some(Subscript::Star);
                self.pos += 3;
            } else {
                self.pos += 1;
                let s = self.pos;
                let mut parts = Vec::new();
                self.parse_parts(Ctx::Subscript, &mut parts)?;
                if self.peek() != Some(b']') {
                    if self.at_eof() {
                        return Err(self.unterminated("}"));
                    }
                    return self.finish_bad_subst(start, dq);
                }
                index = Some(Subscript::Expr(Word {
                    span: sp(s, self.pos),
                    parts,
                }));
                self.pos += 1;
            }
        }
        let operand = |stop: u8| Ctx::Operand { dq, stop };
        let Some(c) = self.peek() else {
            return Err(self.unterminated("}"));
        };
        let op = match c {
            b'}' => ParamOp::None,
            b'*' | b'@'
                if prefix == ParamPrefix::Indirect
                    && name == ParamName::Var
                    && index.is_none()
                    && self.peek_at(1) == Some(b'}') =>
            {
                self.pos += 1;
                ParamOp::Names { star: c == b'*' }
            }
            b':' if matches!(self.peek_at(1), Some(b'-' | b'=' | b'?' | b'+')) => {
                let kind = default_kind(self.b[self.pos + 1]);
                self.pos += 2;
                let word = self.operand_word(operand(0))?;
                ParamOp::Default {
                    kind,
                    colon: true,
                    word,
                }
            }
            b':' => {
                self.pos += 1;
                let offset = self.operand_word(operand(b':'))?;
                let length = if self.peek() == Some(b':') {
                    self.pos += 1;
                    Some(self.operand_word(operand(0))?)
                } else {
                    None
                };
                ParamOp::Substring { offset, length }
            }
            b'-' | b'=' | b'?' | b'+' => {
                self.pos += 1;
                let word = self.operand_word(operand(0))?;
                ParamOp::Default {
                    kind: default_kind(c),
                    colon: false,
                    word,
                }
            }
            b'#' | b'%' => {
                let longest = self.peek_at(1) == Some(c);
                self.pos += if longest { 2 } else { 1 };
                let pattern = self.operand_word(operand(0))?;
                if c == b'#' {
                    ParamOp::RemovePrefix { longest, pattern }
                } else {
                    ParamOp::RemoveSuffix { longest, pattern }
                }
            }
            b'/' => {
                self.pos += 1;
                let kind = match self.peek() {
                    Some(b'/') => {
                        self.pos += 1;
                        ReplaceKind::All
                    }
                    Some(b'#') => {
                        self.pos += 1;
                        ReplaceKind::Prefix
                    }
                    Some(b'%') => {
                        self.pos += 1;
                        ReplaceKind::Suffix
                    }
                    _ => ReplaceKind::First,
                };
                // After `//`, a `/` starts the pattern rather than ending an empty one
                // (`${x////c}` replaces `/` with `c`; bash).
                let slash = (kind == ReplaceKind::All && self.peek() == Some(b'/')).then(|| {
                    self.pos += 1;
                    self.pos - 1
                });
                let mut pattern = self.operand_word(operand(b'/'))?;
                if let Some(at) = slash {
                    let mut parts = Vec::new();
                    push_lit(&mut parts, at, at + 1);
                    for part in core::mem::take(&mut pattern.parts) {
                        match (parts.last_mut(), &part.kind) {
                            (Some(last), Part::Lit)
                                if matches!(last.kind, Part::Lit)
                                    && last.span.end == part.span.start =>
                            {
                                last.span.end = part.span.end;
                            }
                            _ => parts.push(part),
                        }
                    }
                    pattern = Word {
                        span: sp(at, pattern.span.end as usize),
                        parts,
                    };
                }
                let replacement = if self.peek() == Some(b'/') {
                    self.pos += 1;
                    Some(self.operand_word(operand(0))?)
                } else {
                    None
                };
                ParamOp::Replace {
                    kind,
                    pattern,
                    replacement,
                }
            }
            b'^' | b',' | b'~' => {
                let all = self.peek_at(1) == Some(c);
                self.pos += if all { 2 } else { 1 };
                let kind = match (c, all) {
                    (b'^', false) => CaseKind::UpperFirst,
                    (b'^', true) => CaseKind::UpperAll,
                    (b',', false) => CaseKind::LowerFirst,
                    (b',', true) => CaseKind::LowerAll,
                    (_, false) => CaseKind::ToggleFirst,
                    (_, true) => CaseKind::ToggleAll,
                };
                let pattern = self.operand_word(operand(0))?;
                let pattern = if pattern.span.is_empty() {
                    None
                } else {
                    Some(pattern)
                };
                ParamOp::Case { kind, pattern }
            }
            b'@' if self.peek_at(1).is_some_and(|t| t.is_ascii_alphabetic())
                && self.peek_at(2) == Some(b'}') =>
            {
                self.pos += 2;
                ParamOp::Transform(self.b[self.pos - 1])
            }
            _ => return self.finish_bad_subst(start, dq),
        };
        if self.peek() != Some(b'}') {
            if self.at_eof() {
                return Err(self.unterminated("}"));
            }
            return self.finish_bad_subst(start, dq);
        }
        self.pos += 1;
        let valid = match prefix {
            ParamPrefix::Length => matches!(op, ParamOp::None),
            _ => true,
        };
        if !valid {
            return Ok(WordPart {
                span: sp(start, self.pos),
                kind: Part::BadSubst,
            });
        }
        Ok(WordPart {
            span: sp(start, self.pos),
            kind: Part::Param(Box::new(Param {
                braced: true,
                prefix,
                name,
                name_span: sp(ns, ne),
                index,
                op,
            })),
        })
    }

    fn operand_word(&mut self, ctx: Ctx) -> R<Word> {
        let s = self.pos;
        let mut parts = Vec::new();
        self.parse_parts(ctx, &mut parts)?;
        Ok(Word {
            span: sp(s, self.pos),
            parts,
        })
    }

    /// Skips to the `}` closing a `${…}` that is not a valid expansion.
    fn finish_bad_subst(&mut self, start: usize, dq: bool) -> R<WordPart> {
        let mut parts = Vec::new();
        self.parse_parts(Ctx::Operand { dq, stop: 0 }, &mut parts)?;
        if self.peek() != Some(b'}') {
            return Err(self.unterminated("}"));
        }
        self.pos += 1;
        Ok(WordPart {
            span: sp(start, self.pos),
            kind: Part::BadSubst,
        })
    }
}

fn default_kind(c: u8) -> DefaultKind {
    match c {
        b'-' => DefaultKind::Use,
        b'=' => DefaultKind::Assign,
        b'?' => DefaultKind::Error,
        _ => DefaultKind::Alternative,
    }
}
