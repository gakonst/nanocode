//! Turns a tree back into source. Commands are laid out one per line with indentation;
//! words, assignments and arithmetic texts are printed as written (their source text),
//! so nested `$(…)`, quoting and comments inside words survive unchanged.
//!
//! `parse(print(ast))` has the same [`crate::dump`] as `ast`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::ast::*;

/// Prints the whole tree as a script.
pub fn print(ast: &Ast) -> String {
    let mut p = Printer {
        ast,
        out: String::new(),
        indent: 0,
        heredocs: Vec::new(),
    };
    p.list_lines(ast.root());
    p.out
}

struct Printer<'a> {
    ast: &'a Ast,
    out: String,
    indent: usize,
    /// Here-docs whose bodies go after the current line.
    heredocs: Vec<HereDocId>,
}

impl Printer<'_> {
    fn s(&mut self, s: &str) {
        self.out.push_str(s);
    }

    /// Prints source text as written. A here-doc started inside it (in a `$(…)`) whose
    /// body comes after the enclosing line is printed after the line, like the others.
    fn src(&mut self, span: crate::Span) {
        let t = self.ast.text(span);
        self.out.push_str(t);
        for (i, h) in self.ast.heredocs.iter().enumerate() {
            if span.contains(h.delimiter.span) && !span.contains(h.body.span) {
                self.heredocs.push(HereDocId(i as u32));
            }
        }
    }

    fn newline(&mut self) {
        self.out.push('\n');
        let ast = self.ast;
        for id in core::mem::take(&mut self.heredocs) {
            let h = ast.heredoc(id);
            let body = ast.text(h.body.span);
            self.out.push_str(body);
            if h.terminated && !body.is_empty() && !body.ends_with('\n') {
                self.out.push('\n');
            }
            // A body that ran to the end of the input had no delimiter line.
            if h.terminated {
                let delim = heredoc_delim_text(ast, &h.delimiter);
                self.out.push_str(&delim);
                self.out.push('\n');
            }
        }
    }

    fn pad(&mut self) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
    }

    /// Each item of the list on its own line.
    fn list_lines(&mut self, id: ListId) {
        let ast = self.ast;
        let items = &ast.list(id).items;
        for (i, item) in items.iter().enumerate() {
            self.pad();
            self.and_or(item);
            // A here-doc body that runs to the end of the input swallows every line
            // after its operator, so what followed on that line stays on it.
            let open = self.heredocs.iter().any(|&h| !ast.heredoc(h).terminated);
            if open && i + 1 < items.len() {
                if !item.background {
                    self.s(";");
                }
                self.s(" ");
                continue;
            }
            self.newline();
        }
    }

    fn block(&mut self, id: ListId) {
        self.indent += 1;
        self.list_lines(id);
        self.indent -= 1;
    }

    fn and_or(&mut self, ao: &AndOr) {
        self.pipeline(&ao.first);
        for (op, p) in &ao.rest {
            self.s(match op {
                AndOrOp::And => " && ",
                AndOrOp::Or => " || ",
            });
            self.pipeline(p);
        }
        if ao.background {
            self.s(" &");
        }
    }

    fn pipeline(&mut self, p: &Pipeline) {
        let mut sep = "";
        if let Some(t) = p.time {
            self.s(if t.posix { "time -p" } else { "time" });
            sep = " ";
        }
        for _ in 0..p.bangs {
            self.s(sep);
            self.s("!");
            sep = " ";
        }
        for (i, st) in p.stages.iter().enumerate() {
            if i > 0 {
                self.s(if p.stages[i - 1].pipe_stderr {
                    " |& "
                } else {
                    " | "
                });
            } else {
                self.s(sep);
            }
            self.cmd(st.cmd);
        }
    }

    fn close_line(&mut self, word: &str) {
        self.pad();
        self.s(word);
    }

    fn cmd(&mut self, id: CmdId) {
        let ast = self.ast;
        let c = ast.cmd(id);
        match &c.kind {
            CommandKind::Simple(s) => {
                let mut first = true;
                // `<<x case` must not become `case <<x`: a reserved word only stays a
                // plain word behind something else.
                let reserved = s.assigns.is_empty()
                    && s.words.first().is_some_and(|w| {
                        crate::parser::RESERVED.contains(&ast.text(w.span))
                            || ast.text(w.span) == "(("
                    });
                if reserved {
                    for r in &c.redirects {
                        if !first {
                            self.s(" ");
                        }
                        first = false;
                        self.redirect(r);
                    }
                }
                for a in &s.assigns {
                    if !first {
                        self.s(" ");
                    }
                    first = false;
                    self.src(a.span);
                }
                for w in &s.words {
                    if !first {
                        self.s(" ");
                    }
                    first = false;
                    self.src(w.span);
                }
                if !reserved {
                    for r in &c.redirects {
                        if !first {
                            self.s(" ");
                        }
                        first = false;
                        self.redirect(r);
                    }
                }
                return;
            }
            CommandKind::Brace(l) => {
                self.s("{");
                self.newline();
                self.block(*l);
                self.close_line("}");
            }
            CommandKind::Subshell(l) => {
                self.s("(");
                self.newline();
                self.block(*l);
                self.close_line(")");
            }
            CommandKind::If(i) => {
                for (n, b) in i.branches.iter().enumerate() {
                    if n == 0 {
                        self.s("if");
                    } else {
                        self.close_line("elif");
                    }
                    self.newline();
                    self.block(b.cond);
                    self.close_line("then");
                    self.newline();
                    self.block(b.body);
                }
                if let Some(e) = i.else_body {
                    self.close_line("else");
                    self.newline();
                    self.block(e);
                }
                self.close_line("fi");
            }
            CommandKind::While(w) => {
                self.s(if w.until { "until" } else { "while" });
                self.newline();
                self.block(w.cond);
                self.do_group(w.body);
            }
            CommandKind::For(f) | CommandKind::Select(f) => {
                self.s(if matches!(c.kind, CommandKind::For(_)) {
                    "for "
                } else {
                    "select "
                });
                self.src(f.var.span);
                if let Some(words) = &f.words {
                    self.s(" in");
                    for w in words {
                        self.s(" ");
                        self.src(w.span);
                    }
                }
                self.s(";");
                self.newline();
                self.do_group(f.body);
            }
            CommandKind::ArithFor(f) => {
                self.s("for ((");
                self.src(f.init.span);
                self.s(";");
                self.src(f.cond.span);
                self.s(";");
                self.src(f.step.span);
                self.s("))");
                self.newline();
                self.do_group(f.body);
            }
            CommandKind::Case(cs) => {
                self.s("case ");
                self.src(cs.word.span);
                self.s(" in");
                self.newline();
                self.indent += 1;
                for item in &cs.items {
                    self.pad();
                    self.s("(");
                    for (i, p) in item.patterns.iter().enumerate() {
                        if i > 0 {
                            self.s(" | ");
                        }
                        self.src(p.span);
                    }
                    self.s(")");
                    self.newline();
                    if let Some(b) = item.body {
                        self.block(b);
                    }
                    self.indent += 1;
                    self.close_line(match item.terminator {
                        CaseTerminator::Break => ";;",
                        CaseTerminator::FallThrough => ";&",
                        CaseTerminator::Continue => ";;&",
                    });
                    self.indent -= 1;
                    self.newline();
                }
                self.indent -= 1;
                self.close_line("esac");
            }
            CommandKind::Arith(w) => {
                self.s("((");
                self.src(w.span);
                self.s("))");
            }
            CommandKind::Cond(e) => {
                self.s("[[ ");
                self.cond(e);
                self.s(" ]]");
            }
            CommandKind::Function(f) => {
                if f.keyword {
                    self.s("function ");
                    self.src(f.name.span);
                    self.s("() ");
                } else {
                    self.src(f.name.span);
                    self.s("() ");
                }
                self.cmd(f.body);
            }
            CommandKind::Coproc(cp) => {
                self.s("coproc ");
                if let Some(n) = &cp.name {
                    self.src(n.span);
                    self.s(" ");
                }
                self.cmd(cp.body);
            }
        }
        for r in &c.redirects {
            self.s(" ");
            self.redirect(r);
        }
    }

    fn do_group(&mut self, body: ListId) {
        self.close_line("do");
        self.newline();
        self.block(body);
        self.close_line("done");
    }

    fn redirect(&mut self, r: &Redirect) {
        match r.fd {
            Some(RedirFd::Number(n)) => {
                use core::fmt::Write;
                let _ = write!(self.out, "{n}");
            }
            Some(RedirFd::Var(s)) => {
                self.s("{");
                self.src(s);
                self.s("}");
            }
            None => {}
        }
        self.s(r.op.as_str());
        let target = match &r.target {
            RedirTarget::Word(w) => w.span,
            RedirTarget::HereDoc(id) => self.ast.heredoc(*id).delimiter.span,
        };
        // Keep `< <(cmd)` and `<< -EOF` from fusing into other operators.
        if matches!(
            self.ast.text(target).as_bytes().first(),
            Some(b'<' | b'>' | b'-')
        ) {
            self.s(" ");
        }
        match &r.target {
            RedirTarget::Word(w) => self.src(w.span),
            RedirTarget::HereDoc(id) => {
                let ast = self.ast;
                self.s(ast.text(ast.heredoc(*id).delimiter.span));
                self.heredocs.push(*id);
            }
        }
    }

    fn cond(&mut self, e: &CondExpr) {
        match &e.kind {
            CondKind::Unary { op_span, arg, .. } => {
                self.src(*op_span);
                self.s(" ");
                self.src(arg.span);
            }
            CondKind::Binary {
                op_span, lhs, rhs, ..
            } => {
                self.src(lhs.span);
                self.s(" ");
                self.src(*op_span);
                self.s(" ");
                self.src(rhs.span);
            }
            CondKind::Word(w) => self.src(w.span),
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
}

/// The delimiter line of a here-doc: the delimiter word after quote removal.
fn heredoc_delim_text(ast: &Ast, w: &Word) -> String {
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
