//! Word expansion: brace expansion, tilde, parameters, command substitution, word
//! splitting on `IFS`, pathname expansion and quote removal (bash manual, "Shell
//! Expansions"), over the parser's word parts.
//!
//! The expander appends to [`Fields`]: quoted text, unquoted literal text and unquoted
//! expansion results are told apart there, because only the last are split, and only
//! unquoted text keeps its pattern chars for globbing.

use std::rc::Rc;

use psh_syntax::{
    Brace, CaseKind, DefaultKind, ParamName, ParamOp, ParamPrefix, Part, ReplaceKind, Subscript,
    Word, WordPart,
};

use crate::exec;
use crate::options::{SetOpt, Shopt};
use crate::pattern::{self, Pattern, ReplaceMode};
use crate::sched::LocalBoxFuture;
use crate::state::{Code, Unwind, Vp, is_name};

/// One field under construction.
#[derive(Clone, Default, Debug)]
pub(crate) struct Field {
    /// The text after quote removal.
    pub text: Vec<u8>,
    /// The same as a pattern: quoted pattern chars escaped with a backslash.
    pub pat: Vec<u8>,
    /// It has unquoted pattern chars.
    pub glob: bool,
    started: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pending {
    None,
    /// IFS whitespace was seen after text: the next text starts a new field.
    Ws,
    /// A non-whitespace IFS char ended the field.
    Delim,
}

/// The fields of the words being expanded.
pub(crate) struct Fields {
    pub out: Vec<Field>,
    cur: Field,
    pending: Pending,
    ifs: Vec<u8>,
    /// Multibyte chars of IFS (UTF-8 locale): delimiters as a whole, not by byte.
    ifs_mb: Vec<Vec<u8>>,
    /// Word splitting applies (command words); off for assignments, redirection
    /// targets, here-strings, patterns.
    split: bool,
    /// The status of the last command substitution, for `$?` after an assignment-only
    /// command.
    pub subst_status: Option<i32>,
    /// A `patsub_replacement` replacement: quoting escapes only `\` and `&` (D1325).
    rep: bool,
    /// An assignment value: operands (`x=${u-~:~}`) get its tilde rules.
    assign: bool,
}

fn is_ifs_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n')
}

impl Fields {
    pub(crate) fn new(vp: &Vp, split: bool) -> Fields {
        let mut ifs = match vp.var("IFS") {
            Some(v) => v.to_vec(),
            None => b" \t\n".to_vec(),
        };
        let mut ifs_mb = Vec::new();
        if ifs.iter().any(|&b| b >= 0x80) && vp.utf8() {
            let (units, offs) = pattern::units(&ifs, true);
            let mut single = Vec::new();
            for (i, _) in units.iter().enumerate() {
                let ch = &ifs[offs[i]..offs[i + 1]];
                if ch.len() == 1 {
                    single.push(ch[0]);
                } else {
                    ifs_mb.push(ch.to_vec());
                }
            }
            ifs = single;
        }
        Fields {
            out: Vec::new(),
            cur: Field::default(),
            pending: Pending::None,
            ifs,
            ifs_mb,
            split,
            subst_status: None,
            rep: false,
            assign: false,
        }
    }

    fn push_cur(&mut self) {
        let f = std::mem::take(&mut self.cur);
        self.out.push(f);
    }

    fn begin(&mut self) {
        match self.pending {
            Pending::Ws => {
                self.push_cur();
            }
            Pending::Delim | Pending::None => {}
        }
        self.pending = Pending::None;
    }

    /// Quoted text: no splitting, no globbing; even empty, it makes a field.
    pub(crate) fn quoted(&mut self, s: &[u8]) {
        self.begin();
        self.cur.text.extend_from_slice(s);
        if self.rep {
            for &b in s {
                if b == b'\\' || b == b'&' {
                    self.cur.pat.push(b'\\');
                }
                self.cur.pat.push(b);
            }
        } else {
            pattern::escape(s, &mut self.cur.pat);
        }
        self.cur.started = true;
    }

    /// Unquoted text from the source: not split, but its pattern chars are live.
    pub(crate) fn literal(&mut self, s: &[u8]) {
        self.begin();
        self.cur.text.extend_from_slice(s);
        self.cur.pat.extend_from_slice(s);
        if pattern::has_glob_chars(s) {
            self.cur.glob = true;
        }
        self.cur.started = true;
    }

    /// An extglob group (D1321): its text and its pattern, never split.
    pub(crate) fn pattern(&mut self, text: &[u8], pat: &[u8]) {
        self.begin();
        self.cur.text.extend_from_slice(text);
        self.cur.pat.extend_from_slice(pat);
        self.cur.glob = true;
        self.cur.started = true;
    }

    /// The unquoted result of an expansion: split on IFS, pattern chars live.
    pub(crate) fn unquoted(&mut self, s: &[u8]) {
        if !self.split {
            if !s.is_empty() {
                self.literal(s);
            }
            return;
        }
        let mut i = 0;
        while i < s.len() {
            let b = s[i];
            i += 1;
            if b >= 0x80
                && let Some(mb) = self.ifs_mb.iter().find(|m| s[i - 1..].starts_with(m))
            {
                i += mb.len() - 1;
                self.push_cur();
                self.pending = Pending::Delim;
                continue;
            }
            if self.ifs.contains(&b) {
                if is_ifs_ws(b) {
                    if self.pending == Pending::None && self.cur.started {
                        self.pending = Pending::Ws;
                    }
                } else {
                    self.push_cur();
                    self.pending = Pending::Delim;
                }
                continue;
            }
            self.begin();
            self.cur.text.push(b);
            self.cur.pat.push(b);
            if matches!(b, b'*' | b'?' | b'[') {
                self.cur.glob = true;
            }
            self.cur.started = true;
        }
    }

    /// Between the elements of `$@`: a new field.
    fn field_break(&mut self, quoted: bool) {
        if !self.split {
            self.quoted(b" ");
            return;
        }
        if quoted {
            self.begin();
            self.push_cur();
        } else {
            if self.pending == Pending::Ws || (self.pending == Pending::None && self.cur.started) {
                self.push_cur();
            } else {
                self.cur = Field::default();
            }
            self.pending = Pending::None;
        }
    }

    /// Ends a word.
    pub(crate) fn finish_word(&mut self) {
        match self.pending {
            Pending::Ws => self.push_cur(),
            Pending::Delim => self.cur = Field::default(),
            Pending::None => {
                if self.cur.started {
                    self.push_cur();
                } else {
                    self.cur = Field::default();
                }
            }
        }
        self.pending = Pending::None;
    }

    /// All fields joined with spaces (for contexts that want one string).
    pub(crate) fn joined(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, f) in self.out.iter().enumerate() {
            if i > 0 {
                out.push(b' ');
            }
            out.extend_from_slice(&f.text);
        }
        out
    }
}

/// A piece of a brace-expanded word: a part of the source, or text a sequence
/// expression produced (unquoted literal).
enum Piece<'a> {
    Part(&'a WordPart),
    Text(Vec<u8>),
}

/// Brace expansion of a word into piece lists (one per resulting word).
fn brace_expand<'a>(code: &'a Code, parts: &'a [WordPart]) -> Vec<Vec<Piece<'a>>> {
    let mut results: Vec<Vec<Piece<'a>>> = vec![Vec::new()];
    for part in parts {
        match &part.kind {
            Part::Brace(b) => {
                let alts: Vec<Vec<Piece<'a>>> = match &**b {
                    Brace::Alts(words) => words
                        .iter()
                        .flat_map(|w| brace_expand(code, &w.parts))
                        .collect(),
                    Brace::Seq { start, end, step } => {
                        let s = code.ast.text(*start);
                        let e = code.ast.text(*end);
                        let st = step.map(|sp| code.ast.text(sp));
                        match seq(s, e, st) {
                            Some(items) => {
                                items.into_iter().map(|t| vec![Piece::Text(t)]).collect()
                            }
                            None => vec![vec![Piece::Text(code.ast.bytes(part.span).to_vec())]],
                        }
                    }
                };
                let mut next = Vec::with_capacity(results.len() * alts.len());
                for prefix in &results {
                    for alt in &alts {
                        let mut v: Vec<Piece<'a>> = prefix.iter().map(clone_piece).collect();
                        v.extend(alt.iter().map(clone_piece));
                        next.push(v);
                    }
                }
                results = next;
            }
            _ => {
                for r in &mut results {
                    r.push(Piece::Part(part));
                }
            }
        }
    }
    results
}

fn clone_piece<'a>(p: &Piece<'a>) -> Piece<'a> {
    match p {
        Piece::Part(p) => Piece::Part(p),
        Piece::Text(t) => Piece::Text(t.clone()),
    }
}

/// `{x..y[..step]}` with integers or single chars.
fn seq(start: &str, end: &str, step: Option<&str>) -> Option<Vec<Vec<u8>>> {
    // i128, so that steps and ends at the edges of i64 cannot overflow.
    let step_n: i128 = match step {
        // bash leaves a step of i64::MIN (which has no absolute value) unexpanded.
        Some(s) => s.parse::<i64>().ok().filter(|&n| n != i64::MIN)?.into(),
        None => 1,
    };
    let step_abs = step_n.abs().max(1);
    if let (Ok(a), Ok(b)) = (start.parse::<i64>(), end.parse::<i64>()) {
        let (a, b) = (i128::from(a), i128::from(b));
        // Zero padding when either end has a leading zero.
        let pad = |s: &str| {
            let d = s.trim_start_matches('-');
            if d.len() > 1 && d.starts_with('0') {
                s.len()
            } else {
                0
            }
        };
        let width = pad(start).max(pad(end));
        let mut out = Vec::new();
        let mut i = a;
        loop {
            if (a <= b && i > b) || (a > b && i < b) {
                break;
            }
            let s = if width > 0 {
                if i < 0 {
                    format!("-{:0w$}", -i, w = width - 1)
                } else {
                    format!("{:0w$}", i, w = width)
                }
            } else {
                i.to_string()
            };
            out.push(s.into_bytes());
            if out.len() > 1_000_000 {
                return None;
            }
            i = if a <= b { i + step_abs } else { i - step_abs };
        }
        return Some(out);
    }
    let (a, b) = (start.as_bytes(), end.as_bytes());
    if a.len() == 1 && b.len() == 1 && a[0].is_ascii_alphabetic() && b[0].is_ascii_alphabetic() {
        let (a, b) = (i128::from(a[0]), i128::from(b[0]));
        let mut out = Vec::new();
        let mut i = a;
        loop {
            if (a <= b && i > b) || (a > b && i < b) {
                break;
            }
            out.push(vec![i as u8]);
            i = if a <= b { i + step_abs } else { i - step_abs };
        }
        return Some(out);
    }
    None
}

/// Expands command words: brace expansion, all expansions, splitting, globbing.
pub(crate) async fn expand_words(
    vp: &mut Vp,
    code: &Rc<Code>,
    words: &[Word],
) -> Result<Fields, Unwind> {
    let mut f = Fields::new(vp, true);
    for w in words {
        for pieces in brace_expand(code, &w.parts) {
            expand_pieces(vp, code, &pieces, false, &mut f, Ctx::Word).await?;
            f.finish_word();
        }
    }
    glob_fields(vp, &mut f).await?;
    Ok(f)
}

/// Expands a word to one string without splitting or globbing: assignment values,
/// here-strings, `case` words.
pub(crate) async fn expand_single(
    vp: &mut Vp,
    code: &Rc<Code>,
    word: &Word,
    ctx: Ctx,
) -> Result<(Vec<u8>, Option<i32>), Unwind> {
    let mut f = Fields::new(vp, false);
    f.assign = ctx == Ctx::Assign;
    let pieces: Vec<Piece> = word.parts.iter().map(Piece::Part).collect();
    expand_pieces(vp, code, &pieces, false, &mut f, ctx).await?;
    f.finish_word();
    let st = f.subst_status;
    Ok((f.joined(), st))
}

/// Expands a word to a pattern (quoted pattern chars escaped).
pub(crate) async fn expand_pattern(
    vp: &mut Vp,
    code: &Rc<Code>,
    word: &Word,
) -> Result<Vec<u8>, Unwind> {
    let mut f = Fields::new(vp, false);
    let pieces: Vec<Piece> = word.parts.iter().map(Piece::Part).collect();
    expand_pieces(vp, code, &pieces, false, &mut f, Ctx::Word).await?;
    f.finish_word();
    let mut out = Vec::new();
    for (i, field) in f.out.iter().enumerate() {
        if i > 0 {
            out.push(b' ');
        }
        out.extend_from_slice(&field.pat);
    }
    Ok(out)
}

/// A redirection target: brace expansion and globbing apply; it must come out as one
/// word. `Err(None)` is "ambiguous redirect".
pub(crate) async fn expand_target(
    vp: &mut Vp,
    code: &Rc<Code>,
    word: &Word,
) -> Result<Option<Vec<u8>>, Unwind> {
    let f = expand_words(vp, code, std::slice::from_ref(word)).await?;
    if f.out.len() == 1 {
        Ok(Some(
            f.out.into_iter().next().map(|f| f.text).unwrap_or_default(),
        ))
    } else {
        Ok(None)
    }
}

/// The body of an unquoted here-document: expansions, as inside double quotes.
pub(crate) async fn expand_heredoc(
    vp: &mut Vp,
    code: &Rc<Code>,
    body: &Word,
) -> Result<Vec<u8>, Unwind> {
    let mut f = Fields::new(vp, false);
    let pieces: Vec<Piece> = body.parts.iter().map(Piece::Part).collect();
    expand_pieces(vp, code, &pieces, true, &mut f, Ctx::HereDoc).await?;
    f.finish_word();
    Ok(f.joined())
}

/// Where a word is: decides tilde expansion and a few quoting details.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ctx {
    Word,
    /// An assignment value: `~` also after `:`.
    Assign,
    HereDoc,
    /// The word of `${x-word}` and friends: unquoted, its literal text is split like
    /// an expansion's; in double quotes, single quotes are literal (bash).
    Operand,
    /// `"…"` inside the operand of a double-quoted `"${x:+"…"}"`: a backslash quotes
    /// any char there (bash).
    OperandDouble,
}

fn expand_pieces<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    pieces: &'a [Piece<'a>],
    quoted: bool,
    f: &'a mut Fields,
    ctx: Ctx,
) -> LocalBoxFuture<'a, Result<(), Unwind>> {
    Box::pin(async move {
        for (i, piece) in pieces.iter().enumerate() {
            match piece {
                Piece::Text(t) => f.literal(t),
                Piece::Part(p) => {
                    expand_part(vp, code, p, quoted, f, ctx, i == 0).await?;
                }
            }
        }
        Ok(())
    })
}

fn expand_parts<'a>(
    vp: &'a mut Vp,
    code: &'a Rc<Code>,
    parts: &'a [WordPart],
    quoted: bool,
    f: &'a mut Fields,
    ctx: Ctx,
) -> LocalBoxFuture<'a, Result<(), Unwind>> {
    Box::pin(async move {
        for (i, p) in parts.iter().enumerate() {
            expand_part(vp, code, p, quoted, f, ctx, i == 0 && !quoted).await?;
        }
        Ok(())
    })
}

/// The home directory for `~` and the like.
fn tilde(vp: &Vp, name: &[u8]) -> Option<Vec<u8>> {
    match name {
        b"" => vp.var("HOME").map(<[u8]>::to_vec),
        b"+" => vp.var("PWD").map(<[u8]>::to_vec),
        b"-" => vp.var("OLDPWD").map(<[u8]>::to_vec),
        // `~user`, from the host (D1322).
        user if is_name(user)
            || user
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(b)) =>
        {
            vp.host().home_dir(user)
        }
        _ => None,
    }
}

/// Tilde expansion of unquoted literal text at the start of a word (and after `:` in
/// assignments). `first` is whether the text starts the word.
fn literal_with_tilde(vp: &Vp, text: &[u8], first: bool, ctx: Ctx, f: &mut Fields) {
    if ctx == Ctx::HereDoc {
        f.quoted(text);
        return;
    }
    let mut rest = text;
    let mut at_start = first;
    let mut ctx = ctx;
    // A word that looks like an assignment (`x=~/a`, also as a command argument)
    // gets tilde expansion after its `=` and each `:`, as in bash.
    if first
        && let Some(eq) = text.iter().position(|&b| b == b'=')
        && eq > 0
        && is_name(text[..eq].strip_suffix(b"+").unwrap_or(&text[..eq]))
    {
        f.literal(&text[..=eq]);
        rest = &text[eq + 1..];
        ctx = Ctx::Assign;
    }
    loop {
        if at_start && rest.first() == Some(&b'~') {
            let end = rest
                .iter()
                // A `:` ends the prefix in any word (`~:x` is `$HOME:x`, bash).
                .position(|&b| b == b'/' || b == b':')
                .unwrap_or(rest.len());
            // Only a tilde prefix that ends the literal text or a slash counts; a
            // quoted char right after it (`~"x"`) makes it literal, which we cannot
            // see here, so text that ends this part is accepted.
            if let Some(home) = tilde(vp, &rest[1..end]) {
                f.quoted(&home);
                rest = &rest[end..];
            }
        }
        if ctx != Ctx::Assign {
            f.literal(rest);
            return;
        }
        match rest.iter().position(|&b| b == b':') {
            Some(i) => {
                f.literal(&rest[..=i]);
                rest = &rest[i + 1..];
                at_start = true;
            }
            None => {
                f.literal(rest);
                return;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn expand_part(
    vp: &mut Vp,
    code: &Rc<Code>,
    part: &WordPart,
    quoted: bool,
    f: &mut Fields,
    ctx: Ctx,
    first: bool,
) -> Result<(), Unwind> {
    let text = code.ast.bytes(part.span);
    match &part.kind {
        Part::Lit => {
            if ctx == Ctx::OperandDouble && text.contains(&b'\\') {
                // A backslash quotes any char here: `"${v:+"a\xb"}"` is `axb`.
                let mut out = Vec::with_capacity(text.len());
                let mut i = 0;
                while i < text.len() {
                    if text[i] == b'\\' && i + 1 < text.len() {
                        i += 1;
                    }
                    out.push(text[i]);
                    i += 1;
                }
                f.quoted(&out);
            } else if quoted {
                f.quoted(text);
            } else if ctx == Ctx::Operand {
                // `${x-a b}` unquoted: two words.
                if f.assign && text.contains(&b'~') {
                    literal_with_tilde(vp, text, first, Ctx::Assign, f);
                } else if first && text.first() == Some(&b'~') {
                    literal_with_tilde(vp, text, first, Ctx::Word, f);
                } else {
                    f.unquoted(text);
                }
            } else if text.contains(&b'~') || ctx == Ctx::Assign {
                literal_with_tilde(vp, text, first, ctx, f);
            } else {
                f.literal(text);
            }
        }
        Part::Escaped => {
            let c = &text[1..];
            // In double quotes (a `${x-\z}` operand) only `$ ` " \ }` and newline are
            // escapes; the backslash of anything else stays.
            if quoted
                && ctx != Ctx::OperandDouble
                && !matches!(c.first(), Some(b'$' | b'`' | b'"' | b'\\' | b'}' | b'\n'))
            {
                f.quoted(text);
            } else {
                f.quoted(c);
            }
        }
        // `"${x-'b'}"`: the quotes stay.
        Part::Single if quoted && ctx == Ctx::Operand => f.quoted(text),
        Part::Single => f.quoted(&text[1..text.len() - 1]),
        Part::AnsiC => {
            let inner = &text[2..text.len() - 1];
            f.quoted(&psh_syntax::decode_ansi_c(inner));
        }
        Part::Double(inner) | Part::Locale(inner) => {
            // `"…"` makes a field even when everything inside is empty, except that
            // `"$@"` with no parameters makes none.
            if inner.is_empty() || !inner.iter().all(is_at) {
                f.quoted(b"");
            }
            let inner_ctx = if quoted && ctx == Ctx::Operand {
                Ctx::OperandDouble
            } else {
                Ctx::Word
            };
            expand_parts(vp, code, inner, true, f, inner_ctx).await?;
        }
        Part::Param(p) => param(vp, code, p, part, quoted, f).await?,
        Part::BadSubst => {
            vp.error(&format!(
                "{}: bad substitution",
                String::from_utf8_lossy(text)
            ))
            .await;
            return Err(Unwind::Abort(1));
        }
        Part::CmdSubst(list) => {
            let (out, st) = exec::command_subst(vp, code, *list).await?;
            f.subst_status = Some(st);
            emit(f, &out, quoted);
        }
        Part::Backtick => {
            // Unescaped on the UTF-8 view, then back to the source's bytes.
            // In a `${x-…}` operand and a here-document, bash unescapes the body as
            // outside double quotes (`\"` stays).
            let in_dq = quoted && ctx != Ctx::Operand && ctx != Ctx::HereDoc;
            let body = psh_syntax::backtick_body(code.ast.text(part.span), in_dq);
            let mut body = code.ast.restore_bytes(&body).into_owned();
            // A backslash-newline in the body is a line continuation.
            let mut i = 0;
            while i + 1 < body.len() {
                if body[i] == b'\\' && body[i + 1] == b'\n' {
                    body.drain(i..i + 2);
                } else if body[i] == b'\\' {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let (out, st) = exec::command_subst_text(vp, code, &body, part.span.start).await?;
            f.subst_status = Some(st);
            emit(f, &out, quoted);
        }
        Part::Arith(w) => {
            let (text, st) = expand_single(vp, code, w, Ctx::Word).await?;
            if st.is_some() {
                f.subst_status = st;
            }
            match crate::arith::eval(vp, &text) {
                Ok(v) => emit(f, v.to_string().as_bytes(), quoted),
                Err(msg) => {
                    // An error in an arithmetic expansion abandons the command line.
                    vp.error(&msg).await;
                    return Err(Unwind::Abort(1));
                }
            }
        }
        Part::Brace(_) => {
            // Only reached where brace expansion does not apply: the text as is.
            if quoted {
                f.quoted(text);
            } else {
                f.literal(text);
            }
        }
        Part::ProcSubst { output, list } => {
            let path = exec::process_subst(vp, code, *list, *output).await?;
            f.literal(&path);
        }
        Part::FunSub { list, reply } => {
            let (out, st) = exec::fun_subst(vp, code, *list, *reply).await?;
            f.subst_status = Some(st);
            emit(f, &out, quoted);
        }
        Part::ExtGlob { op, alts } => {
            // The pattern `op(alt|alt)` with each alternative expanded as a pattern;
            // the text, should nothing match, is the same with quotes removed.
            let mut pat = vec![*op, b'('];
            let mut txt = vec![*op, b'('];
            for (i, alt) in alts.iter().enumerate() {
                if i > 0 {
                    pat.push(b'|');
                    txt.push(b'|');
                }
                let mut sub = Fields::new(vp, false);
                expand_parts(vp, code, &alt.parts, false, &mut sub, Ctx::Word).await?;
                sub.finish_word();
                for fl in sub.out {
                    pat.extend_from_slice(&fl.pat);
                    txt.extend_from_slice(&fl.text);
                }
            }
            pat.push(b')');
            txt.push(b')');
            f.pattern(&txt, &pat);
        }
        Part::Array(_) => return unsupported(vp, "array assignment").await,
    }
    Ok(())
}

/// `$@`, `${@}`, `${name[@]}`: expansions that may produce no field at all.
fn is_at(p: &WordPart) -> bool {
    match &p.kind {
        Part::Param(p) => {
            let at = p.name == ParamName::Special(b'@') || matches!(p.index, Some(Subscript::At));
            match p.prefix {
                // `"${!a[@]}"`: the keys, also possibly none.
                ParamPrefix::Indirect => {
                    matches!(p.index, Some(Subscript::At)) && matches!(p.op, ParamOp::None)
                }
                ParamPrefix::Length => false,
                ParamPrefix::None => {
                    at && matches!(
                        p.op,
                        ParamOp::None
                            | ParamOp::Default {
                                kind: DefaultKind::Alternative,
                                ..
                            }
                            | ParamOp::Substring { .. }
                            | ParamOp::RemovePrefix { .. }
                            | ParamOp::RemoveSuffix { .. }
                            | ParamOp::Replace { .. }
                            | ParamOp::Case { .. }
                            | ParamOp::Transform(_)
                    )
                }
            }
        }
        _ => false,
    }
}

/// The status a fatal expansion error (`${x?}`, `set -u`) ends the shell with: 127
/// under `-c` unless errexit is on, else 1, as bash 5.3 does.
pub(crate) fn expansion_exit_status(vp: &Vp) -> i32 {
    // Only the shell itself: a subshell (`( )`, `$( )`, a pipeline stage) ends with 1.
    if vp.opts.dash_c && !vp.opts.get(SetOpt::Errexit) && vp.top {
        127
    } else {
        1
    }
}

/// `set -u`: `NAME: unbound variable`, and the shell ends (D1282).
pub(crate) async fn unbound<T>(vp: &Vp, name: &str) -> Result<T, Unwind> {
    vp.error(&format!("{name}: unbound variable")).await;
    Err(Unwind::Exit(expansion_exit_status(vp)))
}

/// A subscript text from a nameref or `unset 'a[…]'`: expanded like a word (quotes
/// removed, `$x` and `$(…)` expanded), as bash does when it reparses it.
pub(crate) async fn expand_sub_text(vp: &mut Vp, sub: &[u8]) -> Result<Vec<u8>, Unwind> {
    if !sub
        .iter()
        .any(|b| matches!(b, b'$' | b'\'' | b'"' | b'\\' | b'`'))
    {
        return Ok(sub.to_vec());
    }
    let mut src = b": ".to_vec();
    src.extend_from_slice(sub);
    let Ok(ast) = psh_syntax::parse_bytes(&src, psh_syntax::Options::default()) else {
        return Ok(sub.to_vec());
    };
    let Some(id) = ast.cmd_ids().next() else {
        return Ok(sub.to_vec());
    };
    let psh_syntax::CommandKind::Simple(sc) = &ast.cmd(id).kind else {
        return Ok(sub.to_vec());
    };
    let Some(w) = sc.words.get(1).cloned() else {
        return Ok(sub.to_vec());
    };
    let code = Rc::new(Code {
        lines: Rc::new(psh_syntax::LineIndex::from_bytes(ast.source_bytes())),
        ast: std::sync::Arc::new(ast),
        line_base: vp.line.saturating_sub(1),
    });
    Ok(expand_single(vp, &code, &w, Ctx::Word).await?.0)
}

pub(crate) async fn unsupported<T>(vp: &Vp, what: &str) -> Result<T, Unwind> {
    vp.error(&format!("{what}: not supported yet")).await;
    // The script stops: what follows likely depends on it (D1057).
    Err(Unwind::Exit(2))
}

fn emit(f: &mut Fields, s: &[u8], quoted: bool) {
    if quoted {
        f.quoted(s);
    } else {
        f.unquoted(s);
    }
}

/// A parameter's value before its operator.
enum Val {
    Unset,
    Str(Vec<u8>),
    /// `$@` (`star` false) or `$*`.
    List(Vec<Vec<u8>>, bool),
}

fn special(vp: &Vp, c: u8) -> Val {
    match c {
        b'?' => Val::Str(vp.status.to_string().into_bytes()),
        b'#' => Val::Str(vp.args.len().to_string().into_bytes()),
        b'$' => match vp.shell_pid {
            Some(pid) => Val::Str(pid.to_string().into_bytes()),
            None => Val::Unset,
        },
        b'!' => match &vp.last_bg {
            Some(job) => Val::Str(job.pid().to_string().into_bytes()),
            None => Val::Unset,
        },
        b'-' => Val::Str(vp.opts.dash()),
        b'0' => Val::Str(vp.arg0.clone()),
        b'@' => Val::List(vp.args.clone(), false),
        b'*' => Val::List(vp.args.clone(), true),
        _ => Val::Unset,
    }
}

fn by_name(vp: &Vp, name: &[u8]) -> Option<Val> {
    if name.iter().all(u8::is_ascii_digit) && !name.is_empty() {
        let n: usize = std::str::from_utf8(name).ok()?.parse().ok()?;
        return Some(if n == 0 {
            Val::Str(vp.arg0.clone())
        } else {
            match vp.args.get(n - 1) {
                Some(v) => Val::Str(v.clone()),
                None => Val::Unset,
            }
        });
    }
    if name.len() == 1 && b"?#$!-@*".contains(&name[0]) {
        return Some(special(vp, name[0]));
    }
    if is_name(name) {
        let name = std::str::from_utf8(name).ok()?;
        return Some(match vp.var(name) {
            Some(v) => Val::Str(v.to_vec()),
            None => Val::Unset,
        });
    }
    None
}

/// The first char of IFS, which joins `"$*"` (a whole multibyte char in UTF-8).
fn ifs_first(vp: &Vp) -> Vec<u8> {
    match vp.var("IFS") {
        Some(v) if v.first().is_some_and(|&b| b >= 0x80) => {
            let (_, offs) = pattern::units(v, vp.utf8());
            v[..offs[1]].to_vec()
        }
        Some(v) => v.first().map(|b| vec![*b]).unwrap_or_default(),
        None => b" ".to_vec(),
    }
}

async fn param(
    vp: &mut Vp,
    code: &Rc<Code>,
    p: &psh_syntax::Param,
    part: &WordPart,
    quoted: bool,
    f: &mut Fields,
) -> Result<(), Unwind> {
    // `$!` names the job's first external: let the job start it (D1900).
    if p.name == ParamName::Special(b'!')
        && let Some(job) = vp.last_bg.clone()
    {
        crate::jobs::settle(&vp.core.sched, &job).await;
    }
    let name_text = code.ast.text(p.name_span).as_bytes();
    let bad = |vp: &Vp| {
        let msg = format!("{}: bad substitution", code.ast.text(part.span));
        let vp = vp.clone();
        async move {
            vp.error(&msg).await;
            Err::<(), Unwind>(Unwind::Abort(1))
        }
    };
    let is_list = matches!(p.index, Some(Subscript::At | Subscript::Star));
    // `${!a[@]}`: the keys of an array.
    if p.prefix == ParamPrefix::Indirect && is_list && p.name == ParamName::Var {
        let name = vp.resolve(code.ast.text(p.name_span)).into_owned();
        let keys = vp.vars.get(&name).map(|v| v.keys()).unwrap_or_default();
        let star = matches!(p.index, Some(Subscript::Star));
        // `${!a[@]-DEF}` of an empty array: no keys is unset.
        if keys.is_empty()
            && let ParamOp::Default { kind, word, .. } = &p.op
        {
            match kind {
                DefaultKind::Use => return expand_operand(vp, code, word, quoted, f).await,
                DefaultKind::Alternative => return Ok(()),
                _ => return bad(vp).await,
            }
        }
        // Unquoted with an empty IFS, `${!a[*]}` is one word, joined by spaces (bash).
        if star && !quoted && f.ifs.is_empty() {
            return emit_val(vp, f, Val::Str(keys.join(&b' ')), quoted);
        }
        return emit_val(vp, f, Val::List(keys, star), quoted);
    }
    // `${x:o:l}` on arrays and `$@` slices by index, so it reads the source itself.
    // `${x:}`: no offset and no length (`${x: }` and `${x::1}` are offset 0).
    if let ParamOp::Substring {
        offset,
        length: None,
    } = &p.op
        && offset.span.start == offset.span.end
    {
        return bad(vp).await;
    }
    if let ParamOp::Substring { offset, length } = &p.op
        && p.prefix == ParamPrefix::None
        && (is_list || matches!(p.name, ParamName::Special(b'@' | b'*')))
        && !(p.name == ParamName::Var && is_scalar(vp, code.ast.text(p.name_span)))
    {
        let off = arith_word(vp, code, offset).await?;
        let len = match length {
            Some(w) => Some(arith_word(vp, code, w).await?),
            None => None,
        };
        let star = matches!(p.index, Some(Subscript::Star)) || p.name == ParamName::Special(b'*');
        let items = if p.name == ParamName::Var {
            let name = vp.resolve(code.ast.text(p.name_span)).into_owned();
            slice_array(vp, &name, off, len)
        } else {
            let mut all = vec![(0, vp.arg0.clone())];
            all.extend(
                vp.args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (i as i64 + 1, a.clone())),
            );
            let past = vp.args.len() as i64 + 1;
            slice_items(&all, off, len, past)
        };
        let items = match items {
            Ok(items) => items,
            Err(msg) => {
                vp.error(&format!("{msg}: substring expression < 0")).await;
                return Err(Unwind::Abort(1));
            }
        };
        return emit_val(vp, f, Val::List(items, star), quoted);
    }
    if matches!(p.name, ParamName::Special(b'$') | ParamName::Var) {
        check_pid(vp, code.ast.text(p.name_span).as_bytes()).await?;
    }
    let mut val = match p.name {
        // `$LINENO`: the line of the command running (in a trap, the line it fired at).
        ParamName::Var if code.ast.text(p.name_span) == "LINENO" && p.index.is_none() => {
            Val::Str(vp.line.to_string().into_bytes())
        }
        ParamName::Var
            if code.ast.text(p.name_span) == "LINENO"
                && matches!(p.index, Some(Subscript::At | Subscript::Star)) =>
        {
            Val::List(
                vec![vp.line.to_string().into_bytes()],
                matches!(p.index, Some(Subscript::Star)),
            )
        }
        ParamName::Var => match vp.target(code.ast.text(p.name_span)) {
            // A nameref to an element (`declare -n r='a[2]'`).
            Ok((name, Some(sub))) if p.index.is_none() => match &sub[..] {
                b"@" | b"*" => {
                    let values = vp.vars.get(&name).map(|v| v.values()).unwrap_or_default();
                    Val::List(values, sub == b"*")
                }
                _ => {
                    let sub = expand_sub_text(vp, &sub).await?;
                    element(vp, &name, &sub).await?
                }
            },
            // A subscript on a reference to an element: nothing (bash).
            Ok((_, Some(_))) => match p.index {
                Some(Subscript::At) => Val::List(Vec::new(), false),
                Some(Subscript::Star) => Val::List(Vec::new(), true),
                _ => Val::Unset,
            },
            Ok((name, None)) => {
                vp.refresh_dynamic(&name);
                lookup_var(vp, code, &name, &p.index).await?
            }
            Err(msg) => {
                vp.error(&format!("warning: {msg}")).await;
                Val::Unset
            }
        },
        ParamName::Positional | ParamName::Special(_) => {
            by_name(vp, name_text).unwrap_or(Val::Unset)
        }
    };

    // `set -u` (D1282): an unset parameter is an error unless an operator handles it.
    if matches!(val, Val::Unset)
        && p.prefix != ParamPrefix::Indirect
        && !matches!(p.op, ParamOp::Default { .. } | ParamOp::Names { .. })
        && vp.opts.get(SetOpt::Nounset)
    {
        let mut name = String::new();
        if !p.braced && p.name != ParamName::Var {
            name.push('$');
        }
        name.push_str(&String::from_utf8_lossy(name_text));
        if let Some(Subscript::Expr(w)) = &p.index {
            name.push_str(&format!("[{}]", code.ast.text(w.span)));
        }
        return unbound(vp, &name).await;
    }
    match p.prefix {
        ParamPrefix::None => {}
        ParamPrefix::Length => {
            let utf8 = vp.utf8();
            let n = match &val {
                Val::Unset => 0,
                Val::Str(s) => pattern::units(s, utf8).0.len(),
                Val::List(l, _) => l.len(),
            };
            emit(f, n.to_string().as_bytes(), quoted);
            return Ok(());
        }
        ParamPrefix::Indirect => {
            if let ParamOp::Names { star } = p.op {
                let prefix = code.ast.text(p.name_span);
                let mut names: Vec<Vec<u8>> = vp
                    .vars
                    .iter()
                    .filter(|(n, v)| n.starts_with(prefix) && v.value != crate::vars::Value::Unset)
                    .map(|(n, _)| n.as_bytes().to_vec())
                    .collect();
                names.sort();
                // Unquoted with an empty IFS, `${!p*}` is one word (bash).
                if star && !quoted && f.ifs.is_empty() {
                    return emit_val(vp, f, Val::Str(names.concat()), quoted);
                }
                return emit_val(vp, f, Val::List(names, star), quoted);
            }
            // A nameref: `${!ref}` is the name it refers to.
            if p.name == ParamName::Var
                && let Some(v) = vp
                    .vars
                    .get(code.ast.text(p.name_span))
                    .filter(|v| v.nameref)
            {
                // Through a chain of namerefs: the last name.
                let first = v.get().map(<[u8]>::to_vec);
                val = match vp.resolve_checked(code.ast.text(p.name_span)) {
                    Ok(n) if first.is_some() => Val::Str(n.as_bytes().to_vec()),
                    _ => first.map_or(Val::Unset, Val::Str),
                };
            } else {
                let target = match &val {
                    Val::Str(s) => s.clone(),
                    Val::Unset => {
                        vp.error(&format!(
                            "{}: invalid indirect expansion",
                            String::from_utf8_lossy(name_text)
                        ))
                        .await;
                        return Err(Unwind::Abort(1));
                    }
                    Val::List(..) => return bad(vp).await,
                };
                val = match indirect(vp, code, &target).await? {
                    Some(Val::Unset)
                        if vp.opts.get(SetOpt::Nounset)
                            && !matches!(p.op, ParamOp::Default { .. }) =>
                    {
                        return unbound(vp, &format!("!{}", String::from_utf8_lossy(name_text)))
                            .await;
                    }
                    Some(v) => v,
                    None => {
                        vp.error(&format!(
                            "{}: invalid variable name",
                            String::from_utf8_lossy(&target)
                        ))
                        .await;
                        return Err(Unwind::Abort(1));
                    }
                };
            }
        }
    }

    let utf8 = vp.utf8();
    let mopts = vp.match_opts(false);
    match &p.op {
        ParamOp::None => emit_val(vp, f, val, quoted),
        ParamOp::Default { kind, colon, word } => {
            let is_set = match &val {
                Val::Unset => false,
                Val::Str(s) => !(*colon && s.is_empty()),
                // `"$*"` with `:` is null when it joins to nothing; `$@` when it has no
                // elements or one empty one.
                Val::List(l, true) if *colon && quoted => !l.join(&ifs_first(vp)[..]).is_empty(),
                Val::List(l, _) => {
                    !l.is_empty() && !(*colon && l.iter().all(|s| s.is_empty()) && l.len() == 1)
                }
            };
            match kind {
                DefaultKind::Use => {
                    if is_set {
                        emit_val(vp, f, val, quoted)
                    } else {
                        expand_operand(vp, code, word, quoted, f).await
                    }
                }
                DefaultKind::Alternative => {
                    if is_set {
                        expand_operand(vp, code, word, quoted, f).await
                    } else {
                        // Quoted, an unset `${x+…}` is an empty word; an empty `$@`
                        // is none.
                        if quoted && !matches!(val, Val::List(_, false)) {
                            f.quoted(b"");
                        }
                        Ok(())
                    }
                }
                DefaultKind::Assign => {
                    if is_set {
                        return emit_val(vp, f, val, quoted);
                    }
                    if p.name != ParamName::Var
                        || matches!(p.index, Some(Subscript::At | Subscript::Star))
                    {
                        vp.error(&format!(
                            "${}: cannot assign in this way",
                            String::from_utf8_lossy(name_text)
                        ))
                        .await;
                        return Err(Unwind::Abort(1));
                    }
                    // The operand as `${x-word}` would expand it, as one word.
                    let v = {
                        let mut sub = Fields::new(vp, false);
                        sub.assign = f.assign;
                        expand_operand(vp, code, word, quoted, &mut sub).await?;
                        sub.finish_word();
                        sub.joined()
                    };
                    let name = code.ast.text(p.name_span);
                    let r = match &p.index {
                        Some(Subscript::Expr(w)) => {
                            let (sub, _) = expand_single(vp, code, w, Ctx::Word).await?;
                            vp.assign_elem(name, &sub, v.clone(), false)
                        }
                        _ => vp.assign(name, v.clone()),
                    };
                    if let Err(msg) = r {
                        vp.error(&msg).await;
                        return Err(Unwind::Abort(1));
                    }
                    // The value as assigned (`-i`, `-u`, … applied).
                    let v = match &p.index {
                        None => vp.var(name).map(<[u8]>::to_vec).unwrap_or(v),
                        _ => v,
                    };
                    emit(f, &v, quoted);
                    Ok(())
                }
                DefaultKind::Error => {
                    if is_set {
                        return emit_val(vp, f, val, quoted);
                    }
                    let (msg, _) = expand_single(vp, code, word, Ctx::Word).await?;
                    let msg = if msg.is_empty() {
                        if *colon {
                            "parameter null or not set".to_string()
                        } else {
                            "parameter not set".to_string()
                        }
                    } else {
                        String::from_utf8_lossy(&msg).into_owned()
                    };
                    vp.error(&format!("{}: {msg}", String::from_utf8_lossy(name_text)))
                        .await;
                    Err(Unwind::Exit(expansion_exit_status(vp)))
                }
            }
        }
        ParamOp::RemovePrefix { longest, pattern } | ParamOp::RemoveSuffix { longest, pattern } => {
            let suffix = matches!(p.op, ParamOp::RemoveSuffix { .. });
            let pat = expand_pattern(vp, code, pattern).await?;
            let val = map_val(val, |s| {
                pattern::remove_affix_with(s, &pat, suffix, *longest, mopts)
            });
            emit_val(vp, f, val, quoted)
        }
        ParamOp::Replace {
            kind,
            pattern,
            replacement,
        } => {
            let pat = expand_pattern(vp, code, pattern).await?;
            // `patsub_replacement`: an unquoted `&` is the match (D1325).
            let rep = match replacement {
                Some(w) if vp.opts.shopt(Shopt::PatsubReplacement) => {
                    let mut f = Fields::new(vp, false);
                    f.rep = true;
                    let pieces: Vec<Piece> = w.parts.iter().map(Piece::Part).collect();
                    expand_pieces(vp, code, &pieces, false, &mut f, Ctx::Word).await?;
                    f.finish_word();
                    let pat: Vec<u8> = f.out.into_iter().flat_map(|x| x.pat).collect();
                    pattern::replacement(&pat)
                }
                Some(w) => vec![pattern::Rep::Lit(
                    expand_single(vp, code, w, Ctx::Word).await?.0,
                )],
                None => Vec::new(),
            };
            let mode = match kind {
                ReplaceKind::First => ReplaceMode::First,
                ReplaceKind::All => ReplaceMode::All,
                ReplaceKind::Prefix => ReplaceMode::Prefix,
                ReplaceKind::Suffix => ReplaceMode::Suffix,
            };
            // `nocasematch` applies to `${x/…}` but not to `${x#…}` (bash).
            let rep_opts = vp.match_opts(vp.opts.shopt(Shopt::Nocasematch));
            let val = map_val(val, |s| {
                if pat.is_empty() && mode != ReplaceMode::Prefix && mode != ReplaceMode::Suffix {
                    s.to_vec()
                } else {
                    pattern::replace_with(s, &pat, &rep, mode, rep_opts)
                }
            });
            emit_val(vp, f, val, quoted)
        }
        ParamOp::Case { kind, pattern } => {
            let pat = match pattern {
                Some(w) => expand_pattern(vp, code, w).await?,
                None => b"?".to_vec(),
            };
            let pat = Pattern::with(&pat, mopts);
            let val = map_val(val, |s| change_case(s, *kind, &pat, utf8));
            emit_val(vp, f, val, quoted)
        }
        ParamOp::Substring { offset, length } => {
            let off = arith_word(vp, code, offset).await?;
            let len = match length {
                Some(w) => Some(arith_word(vp, code, w).await?),
                None => None,
            };
            let mut err = None;
            let val = map_val(val, |s| match substring(s, off, len, utf8) {
                Ok(v) => v,
                Err(e) => {
                    err = Some(e);
                    Vec::new()
                }
            });
            if let Some(e) = err {
                vp.error(&format!("{e}: substring expression < 0")).await;
                return Err(Unwind::Abort(1));
            }
            emit_val(vp, f, val, quoted)
        }
        ParamOp::Transform(op) => {
            let op = *op;
            let any = Pattern::new(b"?", utf8);
            match op {
                b'Q' | b'E' | b'U' | b'L' | b'u' | b'P' => {
                    let val = match (&val, op) {
                        // `@Q` of nothing is nothing, not `''`.
                        (Val::Unset, _) => Val::Unset,
                        _ => map_val(val, |s| match op {
                            b'Q' => quote(s),
                            b'E' => psh_syntax::decode_ansi_c(s),
                            b'U' => change_case(s, CaseKind::UpperAll, &any, utf8),
                            b'L' => change_case(s, CaseKind::LowerAll, &any, utf8),
                            b'P' => prompt(vp, s),
                            _ => change_case(s, CaseKind::UpperFirst, &any, utf8),
                        }),
                    };
                    emit_val(vp, f, val, quoted)
                }
                b'A' | b'a' | b'K' | b'k' => {
                    let val = transform_var(vp, code, p, op, val);
                    emit_val(vp, f, val, quoted)
                }
                _ => bad(vp).await,
            }
        }
        ParamOp::Names { .. } => bad(vp).await,
    }
}

/// `${x@A}`, `@a`, `@K`, `@k`: transformations that need the variable, not only
/// its value.
fn transform_var(vp: &Vp, code: &Rc<Code>, p: &psh_syntax::Param, op: u8, val: Val) -> Val {
    use crate::builtins::declare::{declaration, value_quote};
    let mut name = match p.name {
        ParamName::Var => vp.resolve(code.ast.text(p.name_span)).into_owned(),
        _ => String::new(),
    };
    // `${!ref@a}`: the variable `ref` names.
    if p.prefix == ParamPrefix::Indirect && p.name == ParamName::Var {
        name = vp
            .var(&name)
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .unwrap_or_default();
        name = vp.resolve(&name).into_owned();
    }
    let var = match p.name {
        ParamName::Var => vp.var_ref(&name).cloned(),
        _ => None,
    };
    let list = matches!(p.index, Some(Subscript::At | Subscript::Star));
    let star = matches!(p.index, Some(Subscript::Star));
    match op {
        b'a' => {
            let flags = var.as_ref().map_or(String::new(), |v| {
                let f = v.flags();
                if f == "-" { String::new() } else { f }
            });
            match val {
                Val::List(items, st) => Val::List(
                    items.iter().map(|_| flags.clone().into_bytes()).collect(),
                    st,
                ),
                Val::Unset if var.is_none() => Val::Unset,
                _ => Val::Str(flags.into_bytes()),
            }
        }
        b'A' => {
            let Some(var) = var else {
                return match val {
                    Val::Str(s) if p.name != ParamName::Var => Val::Str(quote(&s)),
                    _ => Val::Unset,
                };
            };
            if list && var.is_array() {
                let mut d = declaration(&name, &var);
                d.pop();
                return Val::Str(d);
            }
            let flags = var.flags();
            let Val::Str(s) = val else {
                // An array without element 0: its declaration alone.
                return if var.is_array() && !list {
                    Val::Str(format!("declare -{flags} {name}").into_bytes())
                } else {
                    Val::Unset
                };
            };
            let mut out = Vec::new();
            if flags != "-" {
                out.extend_from_slice(format!("declare -{flags} ").as_bytes());
            }
            out.extend_from_slice(name.as_bytes());
            out.push(b'=');
            out.extend_from_slice(&quote(&s));
            Val::Str(out)
        }
        _ => {
            // `@K`: key-value pairs as one quoted text; `@k`: separate words.
            let pairs: Vec<(Vec<u8>, Vec<u8>)> = match (&var, list) {
                (Some(v), true) if v.is_array() => v.keys().into_iter().zip(v.values()).collect(),
                _ => {
                    return match val {
                        Val::Str(s) => Val::Str(quote(&s)),
                        Val::List(items, st) => {
                            Val::List(items.iter().map(|s| quote(s)).collect(), st)
                        }
                        other => other,
                    };
                }
            };
            if op == b'k' {
                return Val::List(pairs.into_iter().flat_map(|(k, v)| [k, v]).collect(), star);
            }
            let assoc = var.as_ref().is_some_and(|v| v.is_assoc());
            let mut out = Vec::new();
            for (i, (k, v)) in pairs.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                out.extend_from_slice(k);
                out.push(b' ');
                out.extend_from_slice(&value_quote(v));
            }
            if assoc {
                out.push(b' ');
            }
            Val::Str(out)
        }
    }
}

/// `${x@P}`: the prompt escapes of a non-interactive shell.
fn prompt(vp: &Vp, s: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s[i] != b'\\' || i + 1 >= s.len() {
            out.push(s[i]);
            i += 1;
            continue;
        }
        let c = s[i + 1];
        i += 2;
        match c {
            b'a' => out.push(7),
            b'e' => out.push(0x1b),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b'\\' => out.push(b'\\'),
            b'$' => out.push(b'$'),
            b's' => out.extend_from_slice(vp.msg_name().as_bytes()),
            b'u' => out.extend_from_slice(vp.var("USER").unwrap_or_default()),
            b'w' | b'W' => {
                let pwd = vp.var("PWD").unwrap_or_default().to_vec();
                let home = vp.var("HOME").unwrap_or_default().to_vec();
                let shown = if !home.is_empty() && pwd.starts_with(&home) {
                    let mut v = b"~".to_vec();
                    v.extend_from_slice(&pwd[home.len()..]);
                    v
                } else {
                    pwd
                };
                if c == b'W' {
                    let base = shown
                        .rsplit(|&b| b == b'/')
                        .next()
                        .unwrap_or_default()
                        .to_vec();
                    out.extend_from_slice(if base.is_empty() { &shown } else { &base });
                } else {
                    out.extend_from_slice(&shown);
                }
            }
            b'[' | b']' => {}
            _ => {
                out.push(b'\\');
                out.push(c);
            }
        }
    }
    out
}

/// The value of a variable for `${name}`, `${name[sub]}`, `${name[@]}`.
async fn lookup_var(
    vp: &mut Vp,
    code: &Rc<Code>,
    name: &str,
    index: &Option<Subscript>,
) -> Result<Val, Unwind> {
    match index {
        None => Ok(vp
            .vars
            .value(name)
            .map_or(Val::Unset, |v| Val::Str(v.to_vec()))),
        Some(Subscript::At) | Some(Subscript::Star) => {
            let values = vp.vars.get(name).map(|v| v.values()).unwrap_or_default();
            Ok(Val::List(values, matches!(index, Some(Subscript::Star))))
        }
        Some(Subscript::Expr(w)) => {
            let (sub, _) = expand_single(vp, code, w, Ctx::Word).await?;
            element(vp, name, &sub).await
        }
    }
}

/// `name[sub]` with `sub` expanded: an associative key or an arithmetic index.
async fn element(vp: &mut Vp, name: &str, sub: &[u8]) -> Result<Val, Unwind> {
    match vp.get_elem(name, sub) {
        Ok(v) => Ok(v.map_or(Val::Unset, Val::Str)),
        Err(msg) if msg.ends_with("bad array subscript") => {
            // A reference before the start: an error, but the word goes on empty.
            vp.error(&msg).await;
            Ok(Val::Unset)
        }
        Err(msg) => {
            vp.error(&msg).await;
            Err(Unwind::Abort(1))
        }
    }
}

/// `$$` or `$BASHPID` where psh has no process id to give (D1413): a nested `bash`/`sh`
/// (`$$`, `BASHPID`) or a subshell (`BASHPID`). An expansion error that ends the shell,
/// as `${x:?}` does.
async fn check_pid(vp: &Vp, name: &[u8]) -> Result<(), Unwind> {
    let missing = match name {
        b"$" => vp.shell_pid.is_none(),
        b"BASHPID" => vp.bashpid.is_none(),
        _ => false,
    };
    if !missing {
        return Ok(());
    }
    vp.error(&pid_error(vp, name)).await;
    Err(Unwind::Exit(expansion_exit_status(vp)))
}

/// The message for a process id psh cannot give (D1413).
pub(crate) fn pid_error(vp: &Vp, name: &[u8]) -> String {
    let what = if vp.shell_pid.is_none() {
        "this shell"
    } else {
        "this subshell"
    };
    let name = if name == b"$" { "$$" } else { "BASHPID" };
    format!("{name}: {what} runs inside psh and has no process id")
}

/// `${!name}`: the value of the variable (or `a[sub]`, `a[@]`, positional) named by
/// `target`; `None` if it is not a valid name.
async fn indirect(vp: &mut Vp, code: &Rc<Code>, target: &[u8]) -> Result<Option<Val>, Unwind> {
    check_pid(vp, target).await?;
    if let Some(open) = target.iter().position(|&b| b == b'[')
        && target.ends_with(b"]")
        && is_name(&target[..open])
    {
        let name = vp
            .resolve(&String::from_utf8_lossy(&target[..open]))
            .into_owned();
        let sub = &target[open + 1..target.len() - 1];
        let _ = code;
        return Ok(Some(match sub {
            b"@" | b"*" => {
                let values = vp.vars.get(&name).map(|v| v.values()).unwrap_or_default();
                Val::List(values, sub == b"*")
            }
            _ => {
                let sub = expand_sub_text(vp, sub).await?;
                element(vp, &name, &sub).await?
            }
        }));
    }
    if is_name(target) {
        let name = String::from_utf8_lossy(target).into_owned();
        vp.refresh_dynamic(&name);
        return Ok(Some(
            vp.var(&name).map_or(Val::Unset, |v| Val::Str(v.to_vec())),
        ));
    }
    Ok(by_name(vp, target))
}

/// An arithmetic word (offsets, lengths): expanded, then evaluated.
async fn arith_word(vp: &mut Vp, code: &Rc<Code>, w: &Word) -> Result<i64, Unwind> {
    let (text, _) = expand_single(vp, code, w, Ctx::Word).await?;
    match crate::arith::eval(vp, &text) {
        Ok(v) => Ok(v),
        Err(msg) => {
            vp.error(&msg).await;
            Err(Unwind::Abort(1))
        }
    }
}

/// A set variable that is not an array (`${s[@]:1:2}` slices its string, bash), LINENO
/// included.
fn is_scalar(vp: &mut Vp, name: &str) -> bool {
    if name == "LINENO" {
        return true;
    }
    let name = vp.resolve(name).into_owned();
    vp.refresh_dynamic(&name);
    matches!(
        vp.vars.get(&name).map(|v| &v.value),
        Some(crate::vars::Value::Str(_))
    )
}

/// `${a[@]:off:len}`: elements by index (`off` counts from after the last index when
/// negative), at most `len` of them.
fn slice_array(vp: &Vp, name: &str, off: i64, len: Option<i64>) -> Result<Vec<Vec<u8>>, String> {
    let items: Vec<(i64, Vec<u8>)> = match vp.vars.get(name).map(|v| &v.value) {
        Some(crate::vars::Value::Indexed(m)) => m.iter().map(|(k, v)| (*k, v.clone())).collect(),
        // bash numbers the elements of an associative array from 1, like positional
        // parameters, and offset 0 starts at the first.
        Some(crate::vars::Value::Assoc(a)) => {
            let items: Vec<(i64, Vec<u8>)> = a
                .iter()
                .enumerate()
                .map(|(i, (_, v))| (i as i64 + 1, v.to_vec()))
                .collect();
            let past = items.len() as i64;
            return slice_items(&items, if off == 0 { 1 } else { off }, len, past);
        }
        _ => Vec::new(),
    };
    let past = items.last().map_or(0, |(k, _)| *k);
    slice_items(&items, off, len, past)
}

/// A slice of numbered items. A negative length is an error, unless the slice is empty
/// anyway: no items, a negative offset before the first, or an offset beyond `past`
/// (bash: the highest subscript of an indexed array, the count of an associative one,
/// one more than `$#` for the positional parameters).
pub(crate) fn slice_items(
    items: &[(i64, Vec<u8>)],
    off: i64,
    len: Option<i64>,
    past: i64,
) -> Result<Vec<Vec<u8>>, String> {
    let start = if off < 0 {
        // One past the last index; bash's arithmetic wraps when that is i64::MAX + 1,
        // and a negative offset then selects nothing.
        let end = items.last().map_or(0, |(k, _)| k.wrapping_add(1));
        if end < 0 {
            return Ok(Vec::new());
        }
        end.saturating_add(off)
    } else {
        off
    };
    if off < 0 && start < 0 || items.is_empty() || start > past {
        return Ok(Vec::new());
    }
    if let Some(l) = len
        && l < 0
    {
        return Err(l.to_string());
    }
    let it = items
        .iter()
        .filter(|(k, _)| *k >= start)
        .map(|(_, v)| v.clone());
    Ok(match len {
        Some(l) => it.take(l as usize).collect(),
        None => it.collect(),
    })
}

/// `${x:off:len}` on a string, by chars (UTF-8 locale) or bytes.
fn substring(s: &[u8], off: i64, len: Option<i64>, utf8: bool) -> Result<Vec<u8>, String> {
    let (u, offs) = pattern::units(s, utf8);
    let n = u.len() as i64;
    let start = if off < 0 { n.saturating_add(off) } else { off };
    if start < 0 || start > n {
        return Ok(Vec::new());
    }
    let end = match len {
        None => n,
        Some(l) if l < 0 => {
            let e = n.saturating_add(l);
            if e < start {
                return Err(l.to_string());
            }
            e
        }
        Some(l) => start.saturating_add(l).min(n),
    };
    Ok(s[offs[start as usize]..offs[end as usize]].to_vec())
}

/// `${x@Q}`: quoted for reuse as shell input.
pub(crate) fn quote(s: &[u8]) -> Vec<u8> {
    // Control chars and invalid UTF-8 need `$'…'`; a lone quote is `\'` (bash).
    if s.iter().any(|&b| b.is_ascii_control()) || std::str::from_utf8(s).is_err() {
        return crate::builtins::set::ansi_c_quote(s);
    }
    if s == b"'" {
        return b"\\'".to_vec();
    }
    let mut out = vec![b'\''];
    for &b in s {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

fn change_case(s: &[u8], kind: CaseKind, pat: &Pattern, utf8: bool) -> Vec<u8> {
    let text = String::from_utf8_lossy(s);
    let mut out = String::new();
    let all = matches!(
        kind,
        CaseKind::UpperAll | CaseKind::LowerAll | CaseKind::ToggleAll
    );
    for (i, ch) in text.chars().enumerate() {
        let mut buf = [0u8; 4];
        let applies = (all || i == 0) && pat.matches(ch.encode_utf8(&mut buf).as_bytes(), utf8);
        if !applies {
            out.push(ch);
            continue;
        }
        // Like towupper/towlower: only one-char mappings (ß stays ß).
        let one = |m: &mut dyn Iterator<Item = char>| {
            let v: Vec<char> = m.collect();
            if v.len() == 1 { v[0] } else { ch }
        };
        let up = || one(&mut ch.to_uppercase());
        let down = || one(&mut ch.to_lowercase());
        out.push(match kind {
            CaseKind::UpperFirst | CaseKind::UpperAll => up(),
            CaseKind::LowerFirst | CaseKind::LowerAll => down(),
            CaseKind::ToggleFirst | CaseKind::ToggleAll => {
                if ch.is_uppercase() {
                    down()
                } else {
                    up()
                }
            }
        });
    }
    out.into_bytes()
}

fn map_val(val: Val, mut f: impl FnMut(&[u8]) -> Vec<u8>) -> Val {
    match val {
        Val::Unset => Val::Unset,
        Val::Str(s) => Val::Str(f(&s)),
        Val::List(l, star) => Val::List(l.iter().map(|s| f(s)).collect(), star),
    }
}

fn emit_val(vp: &Vp, f: &mut Fields, val: Val, quoted: bool) -> Result<(), Unwind> {
    match val {
        Val::Unset => {
            if quoted {
                f.quoted(b"");
            }
        }
        Val::Str(s) => emit(f, &s, quoted),
        Val::List(items, star) => {
            if star && (quoted || !f.split) {
                let sep = ifs_first(vp);
                let joined = items.join(&sep[..]);
                emit(f, &joined, quoted);
            } else {
                // Unquoted, an empty element is an empty word unless IFS has
                // whitespace or is empty, which drops it, or it is the last (bash).
                let keep_empty =
                    !quoted && f.split && !f.ifs.is_empty() && !f.ifs.iter().any(|&b| is_ifs_ws(b));
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.field_break(quoted);
                    }
                    if item.is_empty() && keep_empty && i + 1 < items.len() {
                        f.quoted(b"");
                    } else {
                        emit(f, item, quoted);
                    }
                }
            }
        }
    }
    Ok(())
}

/// The word of `${x:-word}` and friends, in the quoting context of the expansion.
async fn expand_operand(
    vp: &mut Vp,
    code: &Rc<Code>,
    word: &Word,
    quoted: bool,
    f: &mut Fields,
) -> Result<(), Unwind> {
    if quoted && word.parts.is_empty() {
        f.quoted(b"");
        return Ok(());
    }
    expand_parts(vp, code, &word.parts, quoted, f, Ctx::Operand).await
}

/// Pathname expansion of the fields with unquoted pattern chars; `set -f`, `nullglob`
/// and `failglob` (D1280).
async fn glob_fields(vp: &mut Vp, f: &mut Fields) -> Result<(), Unwind> {
    if !f.out.iter().any(|x| x.glob) || vp.opts.get(SetOpt::Noglob) {
        return Ok(());
    }
    let fields = std::mem::take(&mut f.out);
    for field in fields {
        if !field.glob {
            f.out.push(field);
            continue;
        }
        let matches = glob(vp, &field.pat).await;
        if matches.is_empty() && is_pattern(&field.pat) {
            if vp.opts.shopt(Shopt::Failglob) {
                vp.error(&format!(
                    "no match: {}",
                    String::from_utf8_lossy(&field.text)
                ))
                .await;
                return Err(Unwind::Abort(1));
            }
            if !vp.opts.shopt(Shopt::Nullglob) {
                f.out.push(field);
            }
        } else if matches.is_empty() {
            f.out.push(field);
        } else {
            for m in matches {
                f.out.push(Field {
                    pat: Vec::new(),
                    text: m,
                    glob: false,
                    started: true,
                });
            }
        }
    }
    Ok(())
}

/// Whether a glob pattern can match anything but itself: an unescaped `*` or `?`, or a
/// `[` with a `]` after it (a lone `[` is an ordinary char). For `nullglob`/`failglob`.
fn is_pattern(pat: &[u8]) -> bool {
    let mut i = 0;
    while i < pat.len() {
        match pat[i] {
            b'\\' => i += 2,
            b'*' | b'?' => return true,
            b'[' if pat[i + 1..].contains(&b']') => return true,
            _ => i += 1,
        }
    }
    false
}

/// Unescapes a pattern component that has no pattern chars.
fn unescape(p: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        if p[i] == b'\\' && i + 1 < p.len() {
            out.push(p[i + 1]);
            i += 2;
        } else {
            out.push(p[i]);
            i += 1;
        }
    }
    out
}

/// The paths matching a pattern, sorted; empty if none.
pub(crate) async fn glob(vp: &Vp, pat: &[u8]) -> Vec<Vec<u8>> {
    let opts = vp.match_opts(vp.opts.shopt(Shopt::Nocaseglob));
    // A non-empty GLOBIGNORE turns dotglob on (bash).
    let ignore: Vec<Pattern> = vp
        .var("GLOBIGNORE")
        .filter(|v| !v.is_empty())
        .map(|v| {
            v.split(|&b| b == b':')
                .filter(|p| !p.is_empty())
                .map(|p| Pattern::with(p, opts))
                .collect()
        })
        .unwrap_or_default();
    let globignore = vp.var("GLOBIGNORE").is_some_and(|v| !v.is_empty());
    let dotglob = vp.opts.shopt(Shopt::Dotglob) || globignore;
    let globstar = vp.opts.shopt(Shopt::Globstar);
    let absolute = pat.first() == Some(&b'/');
    let comps: Vec<&[u8]> = pat
        .split(|&b| b == b'/')
        .filter(|c| !c.is_empty())
        .collect();
    let join = |a: &[u8], b: &[u8]| {
        let mut v = a.to_vec();
        if !v.is_empty() && v.last() != Some(&b'/') {
            v.push(b'/');
        }
        v.extend_from_slice(b);
        v
    };
    let is_dir = |p: Vec<u8>| async move {
        vp.host()
            .stat(&p, true)
            .await
            .is_ok_and(|m| m.kind == crate::host::FileKind::Dir)
    };
    // (shown path, absolute path)
    let mut cur: Vec<(Vec<u8>, Vec<u8>)> = vec![if absolute {
        (b"/".to_vec(), b"/".to_vec())
    } else {
        (Vec::new(), vp.cwd.clone())
    }];
    for (i, comp) in comps.iter().enumerate() {
        let last = i + 1 == comps.len();
        let mut next = Vec::new();
        if globstar && *comp == b"**" {
            // `**`: this directory and every one below it (and, last, every file).
            let mut stack: Vec<(Vec<u8>, Vec<u8>)> = cur.clone();
            while let Some((shown, abs)) = stack.pop() {
                let Ok(mut entries) = vp.host().read_dir(&abs).await else {
                    continue;
                };
                entries.sort_by(|a, b| a.name.cmp(&b.name));
                for e in entries.into_iter().rev() {
                    if e.name.first() == Some(&b'.') && !dotglob {
                        continue;
                    }
                    let full = join(&abs, &e.name);
                    let shown_e = join(&shown, &e.name);
                    // Symlinks to directories are not followed (bash).
                    let dir = vp
                        .host()
                        .stat(&full, false)
                        .await
                        .is_ok_and(|m| m.kind == crate::host::FileKind::Dir);
                    if dir {
                        stack.push((shown_e.clone(), full.clone()));
                    }
                    // Last, `**` is every file too, unless the pattern ends in `/`.
                    if dir || (last && !pat.ends_with(b"/")) {
                        next.push((shown_e, full));
                    }
                }
            }
            // Zero levels: the starting directories themselves (`a/**` lists `a/`;
            // the current directory is never listed).
            for c in &cur {
                if !c.0.is_empty() {
                    let mut shown = c.0.clone();
                    if last && shown.last() != Some(&b'/') {
                        shown.push(b'/');
                    }
                    next.push((shown, c.1.clone()));
                } else if !last {
                    next.push(c.clone());
                }
            }
        } else if !pattern::has_glob_chars_ext(comp, opts.extglob) {
            let name = unescape(comp);
            for (shown, abs) in &cur {
                let p = join(abs, &name);
                if last || is_dir(p.clone()).await {
                    next.push((join(shown, &name), p));
                }
            }
            if last {
                // The final literal component must exist.
                let mut kept = Vec::new();
                for (s, p) in next {
                    if vp.host().stat(&p, false).await.is_ok() {
                        kept.push((s, p));
                    }
                }
                next = kept;
            }
        } else {
            let p = Pattern::with(comp, opts);
            let dot_ok = comp.first() == Some(&b'.') || dotglob;
            // `dir/*/`: only directories.
            let need_dir = !last || pat.ends_with(b"/");
            // `shopt -u globskipdots`: `.*` also matches `.` and `..`.
            let dots = comp.first() == Some(&b'.') && !vp.opts.shopt(Shopt::Globskipdots);
            for (shown, abs) in &cur {
                let Ok(mut entries) = vp.host().read_dir(abs).await else {
                    continue;
                };
                if dots {
                    for d in [&b"."[..], b".."] {
                        entries.push(crate::host::DirEntry { name: d.to_vec() });
                    }
                }
                entries.sort_by(|a, b| a.name.cmp(&b.name));
                for e in entries {
                    if e.name.first() == Some(&b'.') && !dot_ok {
                        continue;
                    }
                    if !p.matches(&e.name, opts.utf8) {
                        continue;
                    }
                    let full = join(abs, &e.name);
                    if need_dir && !is_dir(full.clone()).await {
                        continue;
                    }
                    next.push((join(shown, &e.name), full));
                }
            }
        }
        cur = next;
        if cur.is_empty() {
            return Vec::new();
        }
    }
    let mut out: Vec<Vec<u8>> = cur.into_iter().map(|(s, _)| s).collect();
    if globignore {
        out.retain(|o| {
            let base = o.rsplit(|&b| b == b'/').next().unwrap_or(o);
            base != b"." && base != b".." && !ignore.iter().any(|p| p.matches(o, opts.utf8))
        });
    }
    if pat.ends_with(b"/") {
        for o in &mut out {
            if o.last() != Some(&b'/') {
                o.push(b'/');
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Assignment values, for exec.
pub(crate) async fn assign_value(
    vp: &mut Vp,
    code: &Rc<Code>,
    word: &Word,
) -> Result<(Vec<u8>, Option<i32>), Unwind> {
    expand_single(vp, code, word, Ctx::Assign).await
}

/// The elements of a compound assignment `(…)`: `[key]=value` elements are expanded
/// like assignment values; plain elements like command words (split, globbed), each
/// field an element.
pub(crate) async fn expand_array_lit(
    vp: &mut Vp,
    code: &Rc<Code>,
    lit: &psh_syntax::ArrayLit,
) -> Result<(Vec<crate::arrays::ArrayItem>, Option<i32>), Unwind> {
    let mut items = Vec::new();
    let mut status = None;
    for e in &lit.elems {
        match &e.key {
            Some(k) => {
                let (key, st1) = expand_single(vp, code, k, Ctx::Word).await?;
                let (value, st2) = expand_single(vp, code, &e.value, Ctx::Assign).await?;
                status = st2.or(st1).or(status);
                items.push(crate::arrays::ArrayItem {
                    key: Some(key),
                    value,
                    append: e.append,
                });
            }
            None => {
                let f = expand_words(vp, code, std::slice::from_ref(&e.value)).await?;
                status = f.subst_status.or(status);
                for field in f.out {
                    items.push(crate::arrays::ArrayItem {
                        key: None,
                        value: field.text,
                        append: false,
                    });
                }
            }
        }
    }
    Ok((items, status))
}

/// Whether a word starts like an assignment: `name=`, `name+=` or `name[`.
fn assignment_like(code: &Code, w: &Word) -> bool {
    let Some(p) = w.parts.first() else {
        return false;
    };
    if !matches!(p.kind, Part::Lit) {
        return false;
    }
    let t = code.ast.bytes(p.span);
    let end = t
        .iter()
        .position(|&b| b == b'=' || b == b'[' || b == b'+')
        .unwrap_or(t.len());
    end > 0
        && end < t.len()
        && is_name(&t[..end])
        && (t[end] != b'+' || t.get(end + 1) == Some(&b'='))
}

/// The arguments of a declaration builtin (`declare a=(…) b=$x c`): assignments are
/// expanded like assignment values (no splitting), `name=(…)` into array items, the
/// rest like command words. Returns the builtin's name, its arguments and the status of
/// the last command substitution.
pub(crate) async fn expand_decl_args(
    vp: &mut Vp,
    code: &Rc<Code>,
    words: &[Word],
) -> Result<(Vec<u8>, Vec<crate::builtins::declare::DeclArg>, Option<i32>), Unwind> {
    use crate::builtins::declare::DeclArg;
    let mut args = Vec::new();
    let mut status = None;
    let name = expand_single(vp, code, &words[0], Ctx::Word).await?.0;
    for w in &words[1..] {
        if let Some((i, Part::Array(lit))) =
            w.parts.iter().enumerate().find_map(|(i, p)| match &p.kind {
                Part::Array(l) => Some((i, Part::Array(l.clone()))),
                _ => None,
            })
        {
            // `name=(…)` / `name+=(…)`: the text before the parentheses names it.
            let mut lhs = Vec::new();
            for p in &w.parts[..i] {
                lhs.extend_from_slice(code.ast.bytes(p.span));
            }
            let lhs = lhs.strip_suffix(b"=").unwrap_or(&lhs).to_vec();
            let (lhs, append) = match lhs.strip_suffix(b"+") {
                Some(l) => (l.to_vec(), true),
                None => (lhs, false),
            };
            let (items, st) = expand_array_lit(vp, code, &lit).await?;
            status = st.or(status);
            args.push(DeclArg::Array {
                name: lhs,
                append,
                items,
            });
        } else if assignment_like(code, w) {
            let (v, st) = expand_single(vp, code, w, Ctx::Assign).await?;
            status = st.or(status);
            args.push(DeclArg::from_word(v));
        } else {
            let f = expand_words(vp, code, std::slice::from_ref(w)).await?;
            status = f.subst_status.or(status);
            args.extend(f.out.into_iter().map(|f| DeclArg::from_word(f.text)));
        }
    }
    Ok((name, args, status))
}
