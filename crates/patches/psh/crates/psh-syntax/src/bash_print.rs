//! Bash's own layout of commands, as `declare -f` and `type` print functions.
//!
//! The rules were taken from bash 5.3's output (never its source): four spaces per
//! level, `;` after every command of `if`/`while`/`for` bodies, `do` on its own line
//! after `for`, `elif` printed as a nested `if`, redirections normalized (`>&2` becomes
//! `1>&2`), `$'…'` written as a single-quoted string, here-doc bodies after the
//! operator that follows the command. Inside `$(…)` bash keeps the separators the
//! source had (`;` or newline) and prints braces on one line.

use alloc::string::String;
use alloc::vec::Vec;

use crate::ast::*;
use crate::words::decode_ansi_c;

/// A function definition as `declare -f` prints it (no trailing newline).
pub fn print_function_bash(ast: &Ast, id: CmdId) -> String {
    let mut p = BashPrinter::new(ast, false);
    let c = ast.cmd(id);
    match &c.kind {
        CommandKind::Function(f) => {
            p.word(&f.name);
            p.s(" () \n");
            p.function_body(f.body, 0);
        }
        _ => p.cmd(id, 0),
    }
    p.end_list();
    p.out
}

struct BashPrinter<'a> {
    ast: &'a Ast,
    out: String,
    /// Inside `$(…)`: separators as written, braces on one line.
    inline: bool,
    /// Inside the body of a substitution, where bash keeps newline separators as such
    /// (elsewhere its parser turns them into `;`).
    in_subst: bool,
    heredocs: Vec<HereDocId>,
    /// Here-doc bodies were written and no list separator, brace group end or
    /// redirection has followed yet. Bash then leaves out the `;` of the next separator
    /// (`a\n    b` instead of `a;\n    b`) and writes `while a;    do` for the next loop
    /// or `if` header.
    hd: bool,
}

impl<'a> BashPrinter<'a> {
    fn new(ast: &'a Ast, inline: bool) -> BashPrinter<'a> {
        BashPrinter {
            ast,
            out: String::new(),
            inline,
            in_subst: inline,
            heredocs: Vec::new(),
            hd: false,
        }
    }

    fn s(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn text(&mut self, span: crate::Span) {
        let t = self.ast.text(span);
        self.s(t);
    }

    fn pad(&mut self, level: usize) {
        for _ in 0..level {
            self.s("    ");
        }
    }

    /// Writes pending here-doc bodies, each followed by its delimiter line.
    fn flush(&mut self) {
        if self.heredocs.is_empty() {
            return;
        }
        let ast = self.ast;
        self.out.push('\n');
        for id in core::mem::take(&mut self.heredocs) {
            let h = ast.heredoc(id);
            for p in &h.body.parts {
                self.out.push_str(ast.text(p.span));
            }
            self.out.push_str(&delimiter_text(ast, &h.delimiter));
            self.out.push('\n');
        }
        self.hd = true;
    }

    /// The end of a list: pending here-doc bodies go out now.
    fn end_list(&mut self) {
        self.flush();
    }

    /// The `;` after the last command of a loop or `if` body; here-doc bodies of that
    /// command take its place.
    fn body_semi(&mut self, background: bool) {
        if background {
            self.flush();
        } else if self.heredocs.is_empty() {
            self.s(";");
        } else {
            self.flush();
        }
    }

    /// `; then`, `; do` after a condition.
    fn cond_end(&mut self, word: &str, level: usize) {
        if !self.heredocs.is_empty() {
            // The bodies take the place of the `;`.
            self.flush();
            self.hd = false;
            self.pad(level);
            self.s(word);
            return;
        }
        if self.hd {
            self.hd = false;
            self.s(";");
            self.pad(level);
        } else {
            self.s("; ");
        }
        self.s(word);
    }

    /// The separator between two commands of a list that did not end with `&`.
    fn separator(&mut self, newline: bool, level: usize) {
        let newline = newline && self.in_subst;
        if !self.heredocs.is_empty() {
            self.flush();
            if !self.inline || newline {
                self.s("\n");
                self.pad(level);
            } else {
                self.s(" ");
            }
            return;
        }
        let semi = !core::mem::replace(&mut self.hd, false);
        if self.inline {
            match (newline, semi) {
                (true, true) => self.s("\n"),
                (true, false) => {
                    self.s("\n");
                    self.pad(level);
                }
                (false, true) => self.s("; "),
                (false, false) => self.s(" "),
            }
        } else if newline {
            // A kept newline, then the usual line break (one of them when here-doc
            // bodies were just written).
            self.s(if semi { "\n\n" } else { "\n" });
            self.pad(level);
        } else {
            self.s(if semi { ";\n" } else { "\n" });
            self.pad(level);
        }
    }

    // ---- lists ----

    /// Prints the commands of a list; returns whether the last one ended with `&`.
    fn list(&mut self, id: ListId, level: usize) -> bool {
        let ast = self.ast;
        let items = &ast.list(id).items;
        for (i, ao) in items.iter().enumerate() {
            self.and_or(ao, level);
            if i + 1 == items.len() {
                if ao.background {
                    self.s(" &");
                }
                break;
            }
            if ao.background {
                if self.heredocs.is_empty() {
                    self.s(" & ");
                } else {
                    self.s(" &");
                    self.flush();
                    self.s("  ");
                }
            } else {
                self.separator(ao.newline, level);
            }
        }
        items.last().is_some_and(|ao| ao.background)
    }

    /// The commands of an `if`/`while`/`for` body: each followed by `;`.
    fn body(&mut self, id: ListId, level: usize) {
        self.pad(level);
        let bg = self.list(id, level);
        self.body_semi(bg);
        self.s("\n");
    }

    fn and_or(&mut self, ao: &AndOr, level: usize) {
        self.pipeline(&ao.first, level);
        for (op, p) in &ao.rest {
            let op = match op {
                AndOrOp::And => " && ",
                AndOrOp::Or => " || ",
            };
            self.s(op);
            if !self.heredocs.is_empty() {
                self.flush();
                self.s(" ");
            }
            self.pipeline(p, level);
        }
    }

    fn pipeline(&mut self, p: &Pipeline, level: usize) {
        if let Some(t) = p.time {
            self.s(if t.posix { "time -p " } else { "time " });
        }
        for _ in 0..p.bangs {
            self.s("! ");
        }
        for (i, st) in p.stages.iter().enumerate() {
            if i > 0 {
                if self.heredocs.is_empty() {
                    self.s(" | ");
                } else {
                    self.s(" |");
                    self.flush();
                    self.s("  ");
                }
            }
            self.cmd(st.cmd, level);
            if st.pipe_stderr {
                self.s(" 2>&1");
                self.hd = false;
            }
        }
        if p.stages.is_empty() {
            // `time` or `!` alone.
            while self.out.ends_with(' ') {
                self.out.pop();
            }
        }
    }

    // ---- commands ----

    fn cmd(&mut self, id: CmdId, level: usize) {
        let ast = self.ast;
        let c = ast.cmd(id);
        match &c.kind {
            CommandKind::Simple(s) => {
                let mut first = true;
                for a in &s.assigns {
                    if !first {
                        self.s(" ");
                    }
                    first = false;
                    self.assign(a);
                }
                for w in &s.words {
                    if !first {
                        self.s(" ");
                    }
                    first = false;
                    self.word(w);
                }
                for r in &c.redirects {
                    if !first {
                        self.s(" ");
                    }
                    first = false;
                    self.redirect(r);
                }
                return;
            }
            CommandKind::Brace(l) => {
                if self.inline {
                    self.s("{ ");
                    let bg = self.list(*l, level);
                    self.body_semi(bg);
                    self.s(" }");
                    self.hd = false;
                } else {
                    self.s("{ \n");
                    self.pad(level + 1);
                    self.list(*l, level + 1);
                    self.end_list();
                    self.s("\n");
                    self.pad(level);
                    self.s("}");
                    // A brace group ends what a here-doc started (see `hd`).
                    self.hd = false;
                }
            }
            CommandKind::Subshell(l) => {
                self.s("( ");
                self.list(*l, level);
                self.end_list();
                self.s(" )");
                self.hd = false;
            }
            CommandKind::If(i) => self.if_cmd(i, 0, level),
            CommandKind::While(w) => {
                self.s(if w.until { "until " } else { "while " });
                self.list(w.cond, level);
                self.cond_end("do\n", level);
                self.body(w.body, level + 1);
                self.pad(level);
                self.s("done");
            }
            CommandKind::For(f) | CommandKind::Select(f) => {
                self.s(if matches!(c.kind, CommandKind::For(_)) {
                    "for "
                } else {
                    "select "
                });
                self.word(&f.var);
                self.s(" in ");
                match &f.words {
                    Some(words) => {
                        for (i, w) in words.iter().enumerate() {
                            if i > 0 {
                                self.s(" ");
                            }
                            self.word(w);
                        }
                    }
                    None => self.s("\"$@\""),
                }
                self.s(";\n");
                self.do_group(f.body, level);
            }
            CommandKind::ArithFor(f) => {
                self.s("for ((");
                self.arith_clause(&f.init);
                self.s("; ");
                self.arith_clause(&f.cond);
                self.s("; ");
                self.arith_clause(&f.step);
                self.s("))\n");
                self.do_group(f.body, level);
            }
            CommandKind::Case(cs) => {
                self.s("case ");
                self.word(&cs.word);
                self.s(" in ");
                for (i, item) in cs.items.iter().enumerate() {
                    if i > 0 || !self.inline {
                        self.s("\n");
                        self.pad(level + 1);
                    }
                    for (j, p) in item.patterns.iter().enumerate() {
                        if j > 0 {
                            self.s(" | ");
                        }
                        self.word(p);
                    }
                    self.s(")\n");
                    if let Some(b) = item.body {
                        self.pad(level + 2);
                        self.list(b, level + 2);
                        self.end_list();
                    }
                    self.s("\n");
                    self.pad(level + 1);
                    self.s(match item.terminator {
                        CaseTerminator::Break => ";;",
                        CaseTerminator::FallThrough => ";&",
                        CaseTerminator::Continue => ";;&",
                    });
                }
                self.s("\n");
                self.pad(level);
                self.s("esac");
            }
            CommandKind::Arith(w) => {
                self.s("((");
                self.text(w.span);
                self.s("))");
            }
            CommandKind::Cond(e) => {
                self.s("[[ ");
                self.cond(e);
                self.s(" ]]");
            }
            CommandKind::Function(f) => {
                self.s("function ");
                self.word(&f.name);
                self.s(" () \n");
                self.pad(level);
                self.function_body(f.body, level);
            }
            CommandKind::Coproc(cp) => {
                self.s("coproc ");
                let body = ast.cmd(cp.body);
                if !matches!(body.kind, CommandKind::Simple(_)) {
                    match &cp.name {
                        Some(n) => self.word(n),
                        None => self.s("COPROC"),
                    }
                    self.s(" ");
                } else if let Some(n) = &cp.name {
                    self.word(n);
                    self.s(" ");
                }
                self.cmd(cp.body, level);
            }
        }
        for r in &c.redirects {
            self.s(" ");
            self.redirect(r);
        }
    }

    /// `{ … }` of a function: a brace group as is (its redirections after the `}`),
    /// any other compound command inside braces.
    fn function_body(&mut self, body: CmdId, level: usize) {
        let ast = self.ast;
        let b = ast.cmd(body);
        self.s("{ \n");
        self.pad(level + 1);
        match &b.kind {
            CommandKind::Brace(l) => {
                let inline = core::mem::replace(&mut self.inline, false);
                self.list(*l, level + 1);
                self.end_list();
                self.inline = inline;
                self.s("\n");
                self.pad(level);
                self.s("}");
                self.hd = false;
                for r in &b.redirects {
                    self.s(" ");
                    self.redirect(r);
                }
            }
            _ => {
                let inline = core::mem::replace(&mut self.inline, false);
                self.cmd(body, level + 1);
                self.end_list();
                self.inline = inline;
                self.s("\n");
                self.pad(level);
                self.s("}");
            }
        }
    }

    fn if_cmd(&mut self, i: &IfCommand, from: usize, level: usize) {
        let branch = &i.branches[from];
        self.s("if ");
        self.list(branch.cond, level);
        // Bash indents a `then` displaced by here-docs by one level, however deep the
        // `if` is (a `do` gets the loop's own level).
        self.cond_end("then\n", 1);
        self.body(branch.body, level + 1);
        if from + 1 < i.branches.len() {
            // `elif` is printed as `else if … fi`.
            self.pad(level);
            self.s("else\n");
            self.pad(level + 1);
            self.if_cmd(i, from + 1, level + 1);
            self.body_semi(false);
            self.s("\n");
        } else if let Some(e) = i.else_body {
            self.pad(level);
            self.s("else\n");
            self.body(e, level + 1);
        }
        self.pad(level);
        self.s("fi");
    }

    fn do_group(&mut self, body: ListId, level: usize) {
        self.pad(level);
        self.s("do\n");
        self.body(body, level + 1);
        self.pad(level);
        self.s("done");
    }

    /// A clause of `for ((…))`: leading blanks dropped, `1` when empty.
    fn arith_clause(&mut self, w: &Word) {
        let t = self.ast.text(w.span).trim_start_matches([' ', '\t', '\n']);
        if t.is_empty() {
            self.s("1");
        } else {
            self.s(t);
        }
    }

    fn cond(&mut self, e: &CondExpr) {
        match &e.kind {
            CondKind::Unary { op_span, arg, .. } => {
                self.text(*op_span);
                self.s(" ");
                self.word(arg);
            }
            CondKind::Binary {
                op_span, lhs, rhs, ..
            } => {
                self.word(lhs);
                self.s(" ");
                self.text(*op_span);
                self.s(" ");
                self.word(rhs);
            }
            CondKind::Word(w) => {
                self.s("-n ");
                self.word(w);
            }
            CondKind::Not(i) => {
                self.s("! ");
                self.cond(i);
            }
            CondKind::And(a, b) => {
                self.cond(a);
                self.s(" && ");
                self.cond(b);
            }
            CondKind::Or(a, b) => {
                self.cond(a);
                self.s(" || ");
                self.cond(b);
            }
            CondKind::Group(i) => {
                self.s("( ");
                self.cond(i);
                self.s(" )");
            }
        }
    }

    fn redirect(&mut self, r: &Redirect) {
        // Printing a redirection ends what a here-doc started (see `hd`).
        self.hd = false;
        let ast = self.ast;
        let dup = matches!(r.op, RedirOp::DupRead | RedirOp::DupWrite);
        let target_text = match &r.target {
            RedirTarget::Word(w) => ast.text(w.span),
            RedirTarget::HereDoc(_) => "",
        };
        // A duplication of a literal fd (`>&2`, `<&3-`, `>&-`) shows its default fd; one
        // of a word (`>&$fd`) is printed as written.
        let digits = target_text.strip_suffix('-').unwrap_or(target_text);
        let numeric = digits.bytes().all(|c| c.is_ascii_digit());
        // `n<&-` closes like `n>&-`, and bash prints it so.
        let close = dup && target_text == "-";
        let default_fd = match r.op {
            RedirOp::Read | RedirOp::ReadWrite | RedirOp::DupRead => 0,
            RedirOp::HereString | RedirOp::HereDoc | RedirOp::HereDocStrip => 0,
            _ => 1,
        };
        match r.fd {
            Some(RedirFd::Number(n)) if n != default_fd || dup => {
                use core::fmt::Write;
                let _ = write!(self.out, "{n}");
            }
            Some(RedirFd::Number(_)) => {}
            Some(RedirFd::Var(v)) => {
                self.s("{");
                self.text(v);
                self.s("}");
            }
            None if dup && numeric => {
                use core::fmt::Write;
                let _ = write!(self.out, "{default_fd}");
            }
            None => {}
        }
        let op = if close { ">&" } else { r.op.as_str() };
        self.s(op);
        match &r.target {
            RedirTarget::Word(w) => {
                if !dup {
                    self.s(" ");
                }
                self.word(w);
            }
            RedirTarget::HereDoc(id) => {
                let h = ast.heredoc(*id);
                let d = delimiter_text(ast, &h.delimiter);
                if h.quoted {
                    self.s("'");
                    self.s(&d);
                    self.s("'");
                } else {
                    self.s(&d);
                }
                self.heredocs.push(*id);
            }
        }
    }

    // ---- words ----

    fn assign(&mut self, a: &Assign) {
        self.text(a.name);
        if let Some(i) = &a.index {
            self.s("[");
            self.text(i.span);
            self.s("]");
        }
        self.s(if a.append { "+=" } else { "=" });
        match &a.value {
            AssignValue::Scalar(w) => self.word(w),
            AssignValue::Array(arr) => self.array(arr),
        }
    }

    fn array(&mut self, arr: &ArrayLit) {
        self.s("(");
        for (i, e) in arr.elems.iter().enumerate() {
            if i > 0 {
                self.s(" ");
            }
            if let Some(k) = &e.key {
                self.s("[");
                self.text(k.span);
                self.s(if e.append { "]+=" } else { "]=" });
            }
            self.word(&e.value);
        }
        self.s(")");
    }

    fn word(&mut self, w: &Word) {
        self.parts(&w.parts);
    }

    fn parts(&mut self, parts: &[WordPart]) {
        for p in parts {
            self.part(p);
        }
    }

    fn part(&mut self, p: &WordPart) {
        let ast = self.ast;
        match &p.kind {
            Part::Double(inner) | Part::Locale(inner) => {
                self.s("\"");
                self.parts(inner);
                self.s("\"");
            }
            Part::AnsiC => {
                let t = ast.text(p.span).as_bytes();
                let value = decode_ansi_c(&t[2..t.len() - 1]);
                let quoted = single_quote(&value);
                self.s(&quoted);
            }
            // `$((…) )` that turned out not to be arithmetic, and `<((…) )`: bash keeps
            // the text as written.
            Part::CmdSubst(_) | Part::ProcSubst { .. }
                if ast.text(p.span).as_bytes().get(2) == Some(&b'(') =>
            {
                self.verbatim(p.span);
            }
            Part::CmdSubst(l) => {
                let body = self.inline_list(*l);
                self.s("$(");
                // Bash separates a subshell from the `$(` (not an arithmetic command).
                if body.starts_with("( ") {
                    self.s(" ");
                }
                self.s(&body);
                self.s(")");
            }
            Part::ProcSubst { output, list } => {
                let body = self.inline_list(*list);
                self.s(if *output { ">(" } else { "<(" });
                self.s(&body);
                self.s(")");
            }
            Part::FunSub { list, reply } => {
                let body = self.inline_list(*list);
                self.s(if *reply { "${|" } else { "${ " });
                self.s(&body);
                self.s("; }");
            }
            Part::Array(arr) => self.array(arr),
            _ => self.text(p.span),
        }
    }

    /// Source text as written, except for the parts [`collect_quotes`] finds.
    fn verbatim(&mut self, span: crate::Span) {
        let ast = self.ast;
        let mut quotes: Vec<&WordPart> = Vec::new();
        for id in ast.cmd_ids() {
            let c = ast.cmd(id);
            if span.contains(c.span) {
                for_each_word(ast, c, &mut |w| collect_quotes(&w.parts, &mut quotes));
            }
        }
        quotes.sort_by_key(|p| p.span.start);
        let mut at = span.start;
        for q in quotes {
            if q.span.start < at {
                continue;
            }
            self.text(crate::Span::new(at, q.span.start));
            self.part(q);
            at = q.span.end;
        }
        self.text(crate::Span::new(at, span.end));
    }

    /// The body of a substitution, printed on its own.
    fn inline_list(&self, id: ListId) -> String {
        let mut p = BashPrinter::new(self.ast, true);
        p.list(id, 0);
        p.end_list();
        p.out
    }
}

/// Calls `f` for the words of a command itself (not of nested commands).
fn for_each_word<'a>(ast: &'a Ast, c: &'a Command, f: &mut dyn FnMut(&'a Word)) {
    let assign = |a: &'a Assign, f: &mut dyn FnMut(&'a Word)| match &a.value {
        AssignValue::Scalar(w) => f(w),
        AssignValue::Array(arr) => arr.elems.iter().for_each(|e| f(&e.value)),
    };
    match &c.kind {
        CommandKind::Simple(s) => {
            s.assigns.iter().for_each(|a| assign(a, f));
            s.words.iter().for_each(&mut *f);
        }
        CommandKind::For(fc) | CommandKind::Select(fc) => {
            fc.words.iter().flatten().for_each(&mut *f);
        }
        CommandKind::Case(cs) => {
            f(&cs.word);
            for item in &cs.items {
                item.patterns.iter().for_each(&mut *f);
            }
        }
        CommandKind::Cond(e) => cond_words(e, f),
        _ => {}
    }
    for r in &c.redirects {
        if let RedirTarget::Word(w) = &r.target {
            f(w);
        }
    }
    let _ = ast;
}

fn cond_words<'a>(e: &'a CondExpr, f: &mut dyn FnMut(&'a Word)) {
    match &e.kind {
        CondKind::Unary { arg, .. } => f(arg),
        CondKind::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        CondKind::Word(w) => f(w),
        CondKind::Not(i) | CondKind::Group(i) => cond_words(i, f),
        CondKind::And(a, b) | CondKind::Or(a, b) => {
            cond_words(a, f);
            cond_words(b, f);
        }
    }
}

/// The parts bash rewrites even in text it keeps as written: `$'…'`, `$"…"` and
/// command substitutions (which it prints anew), nested ones included.
fn collect_quotes<'a>(parts: &'a [WordPart], out: &mut Vec<&'a WordPart>) {
    for p in parts {
        match &p.kind {
            Part::AnsiC
            | Part::Locale(_)
            | Part::CmdSubst(_)
            | Part::ProcSubst { .. }
            | Part::FunSub { .. } => out.push(p),
            Part::Double(inner) => collect_quotes(inner, out),
            _ => {}
        }
    }
}

/// The delimiter after quote removal.
fn delimiter_text(ast: &Ast, w: &Word) -> String {
    fn walk(ast: &Ast, parts: &[WordPart], out: &mut String) {
        for p in parts {
            let text = ast.text(p.span);
            match &p.kind {
                Part::Escaped => out.push_str(&text[1..]),
                Part::Single => out.push_str(&text[1..text.len() - 1]),
                Part::AnsiC => out.push_str(&String::from_utf8_lossy(
                    &crate::words::decode_ansi_c(&text.as_bytes()[2..text.len() - 1]),
                )),
                Part::Double(inner) | Part::Locale(inner) => walk(ast, inner, out),
                _ => out.push_str(text),
            }
        }
    }
    let mut out = String::new();
    walk(ast, &w.parts, &mut out);
    out
}

/// Bash's single quoting: `'…'` with each `'` as `'\''`; a lone `'` is `\'`.
fn single_quote(value: &[u8]) -> String {
    let value = String::from_utf8_lossy(value);
    if value == "'" {
        return String::from("\\'");
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}
