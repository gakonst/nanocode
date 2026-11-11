//! A compact, span-free S-expression form of the tree, for tests and debugging. Two
//! trees with the same dump have the same structure and text; the round-trip test
//! compares `dump(parse(print(ast)))` with `dump(ast)`.

use alloc::string::String;
use core::fmt::Write;

use crate::ast::*;

/// The whole tree as one line.
pub fn dump(ast: &Ast) -> String {
    let mut d = Dumper {
        ast,
        out: String::new(),
    };
    d.list(ast.root());
    d.out
}

struct Dumper<'a> {
    ast: &'a Ast,
    out: String,
}

impl Dumper<'_> {
    fn s(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn text(&mut self, span: crate::Span) {
        let t = self.ast.text(span);
        let _ = write!(self.out, "{t:?}");
    }

    fn list(&mut self, id: ListId) {
        self.s("(list");
        for item in &self.ast.list(id).items {
            self.s(" ");
            self.and_or(item);
        }
        self.s(")");
    }

    fn and_or(&mut self, ao: &AndOr) {
        if ao.rest.is_empty() && !ao.background {
            return self.pipeline(&ao.first);
        }
        self.s("(andor ");
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
        self.s(")");
    }

    fn pipeline(&mut self, p: &Pipeline) {
        if p.stages.len() == 1 && p.time.is_none() && p.bangs == 0 {
            return self.cmd(p.stages[0].cmd);
        }
        self.s("(pipe");
        if let Some(t) = p.time {
            self.s(if t.posix { " time-p" } else { " time" });
        }
        for _ in 0..p.bangs {
            self.s(" !");
        }
        for (i, st) in p.stages.iter().enumerate() {
            if i > 0 {
                self.s(if p.stages[i - 1].pipe_stderr {
                    " |&"
                } else {
                    " |"
                });
            }
            self.s(" ");
            self.cmd(st.cmd);
        }
        self.s(")");
    }

    fn cmd(&mut self, id: CmdId) {
        let ast = self.ast;
        let c = ast.cmd(id);
        match &c.kind {
            CommandKind::Simple(s) => {
                self.s("(cmd");
                for a in &s.assigns {
                    self.s(" ");
                    self.assign(a);
                }
                for w in &s.words {
                    self.s(" ");
                    self.word(w);
                }
            }
            CommandKind::Brace(l) => {
                self.s("(brace ");
                self.list(*l);
            }
            CommandKind::Subshell(l) => {
                self.s("(subshell ");
                self.list(*l);
            }
            CommandKind::If(i) => {
                self.s("(if");
                for b in &i.branches {
                    self.s(" ");
                    self.list(b.cond);
                    self.s(" ");
                    self.list(b.body);
                }
                if let Some(e) = i.else_body {
                    self.s(" else ");
                    self.list(e);
                }
            }
            CommandKind::While(w) => {
                self.s(if w.until { "(until " } else { "(while " });
                self.list(w.cond);
                self.s(" ");
                self.list(w.body);
            }
            CommandKind::For(f) | CommandKind::Select(f) => {
                self.s(if matches!(c.kind, CommandKind::For(_)) {
                    "(for "
                } else {
                    "(select "
                });
                self.word(&f.var);
                if let Some(words) = &f.words {
                    self.s(" in");
                    for w in words {
                        self.s(" ");
                        self.word(w);
                    }
                    self.s(";");
                }
                self.s(" ");
                self.list(f.body);
            }
            CommandKind::ArithFor(f) => {
                self.s("(arith-for ");
                self.word(&f.init);
                self.s(" ");
                self.word(&f.cond);
                self.s(" ");
                self.word(&f.step);
                self.s(" ");
                self.list(f.body);
            }
            CommandKind::Case(cs) => {
                self.s("(case ");
                self.word(&cs.word);
                for item in &cs.items {
                    self.s(" (");
                    for (i, p) in item.patterns.iter().enumerate() {
                        if i > 0 {
                            self.s(" | ");
                        }
                        self.word(p);
                    }
                    self.s(")");
                    if let Some(b) = item.body {
                        self.s(" ");
                        self.list(b);
                    }
                    self.s(match item.terminator {
                        CaseTerminator::Break => " ;;",
                        CaseTerminator::FallThrough => " ;&",
                        CaseTerminator::Continue => " ;;&",
                    });
                }
            }
            CommandKind::Arith(w) => {
                self.s("(arith ");
                self.word(w);
            }
            CommandKind::Cond(e) => {
                self.s("(cond ");
                self.cond(e);
            }
            CommandKind::Function(f) => {
                self.s(if f.keyword { "(function " } else { "(fn " });
                self.word(&f.name);
                self.s(" ");
                self.cmd(f.body);
            }
            CommandKind::Coproc(cp) => {
                self.s("(coproc ");
                if let Some(n) = &cp.name {
                    self.word(n);
                    self.s(" ");
                }
                self.cmd(cp.body);
            }
        }
        for r in &c.redirects {
            self.s(" ");
            self.redirect(r);
        }
        self.s(")");
    }

    fn redirect(&mut self, r: &Redirect) {
        match r.fd {
            Some(RedirFd::Number(n)) => {
                let _ = write!(self.out, "{n}");
            }
            Some(RedirFd::Var(s)) => {
                self.s("{");
                self.s(self.ast.text(s));
                self.s("}");
            }
            None => {}
        }
        self.s(r.op.as_str());
        match &r.target {
            RedirTarget::Word(w) => self.word(w),
            RedirTarget::HereDoc(id) => {
                let ast = self.ast;
                let h = ast.heredoc(*id);
                self.word(&h.delimiter);
                self.s(if h.quoted { "[quoted " } else { "[" });
                self.parts(&h.body.parts);
                self.s("]");
            }
        }
    }

    fn assign(&mut self, a: &Assign) {
        self.s("(assign ");
        self.s(self.ast.text(a.name));
        if let Some(i) = &a.index {
            self.s("[");
            self.word(i);
            self.s("]");
        }
        self.s(if a.append { "+= " } else { "= " });
        match &a.value {
            AssignValue::Scalar(w) => self.word(w),
            AssignValue::Array(arr) => self.array(arr),
        }
        self.s(")");
    }

    fn array(&mut self, arr: &ArrayLit) {
        self.s("(array");
        for e in &arr.elems {
            self.s(" ");
            if let Some(k) = &e.key {
                self.s("[");
                self.word(k);
                self.s(if e.append { "]+=" } else { "]=" });
            }
            self.word(&e.value);
        }
        self.s(")");
    }

    fn cond(&mut self, e: &CondExpr) {
        match &e.kind {
            CondKind::Unary { op_span, arg, .. } => {
                self.s("(");
                self.s(self.ast.text(*op_span));
                self.s(" ");
                self.word(arg);
                self.s(")");
            }
            CondKind::Binary {
                op_span, lhs, rhs, ..
            } => {
                self.s("(");
                self.word(lhs);
                self.s(" ");
                self.s(self.ast.text(*op_span));
                self.s(" ");
                self.word(rhs);
                self.s(")");
            }
            CondKind::Word(w) => self.word(w),
            CondKind::Not(i) => {
                self.s("(! ");
                self.cond(i);
                self.s(")");
            }
            CondKind::And(a, b) => {
                self.s("(");
                self.cond(a);
                self.s(" && ");
                self.cond(b);
                self.s(")");
            }
            CondKind::Or(a, b) => {
                self.s("(");
                self.cond(a);
                self.s(" || ");
                self.cond(b);
                self.s(")");
            }
            CondKind::Group(i) => {
                self.s("(group ");
                self.cond(i);
                self.s(")");
            }
        }
    }

    fn word(&mut self, w: &Word) {
        self.s("<");
        self.parts(&w.parts);
        self.s(">");
    }

    fn parts(&mut self, parts: &[WordPart]) {
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                self.s(" ");
            }
            self.part(p);
        }
    }

    fn part(&mut self, p: &WordPart) {
        let ast = self.ast;
        match &p.kind {
            Part::Lit => self.text(p.span),
            Part::Escaped | Part::Single | Part::AnsiC | Part::Backtick => {
                self.s(ast.text(p.span));
            }
            Part::Double(inner) => {
                self.s("dq(");
                self.parts(inner);
                self.s(")");
            }
            Part::Locale(inner) => {
                self.s("ldq(");
                self.parts(inner);
                self.s(")");
            }
            Part::Param(pe) => self.param(pe),
            Part::BadSubst => {
                self.s("bad(");
                self.s(ast.text(p.span));
                self.s(")");
            }
            Part::CmdSubst(l) => {
                self.s("$");
                self.list(*l);
            }
            Part::FunSub { list, reply } => {
                self.s(if *reply { "${|" } else { "${ " });
                self.list(*list);
                self.s("}");
            }
            Part::Arith(w) => {
                self.s("arith");
                self.word(w);
            }
            Part::ProcSubst { output, list } => {
                self.s(if *output { ">" } else { "<" });
                self.list(*list);
            }
            Part::Brace(b) => match &**b {
                Brace::Alts(alts) => {
                    self.s("brace(");
                    for (i, a) in alts.iter().enumerate() {
                        if i > 0 {
                            self.s(",");
                        }
                        self.word(a);
                    }
                    self.s(")");
                }
                Brace::Seq { start, end, step } => {
                    self.s("seq(");
                    self.s(ast.text(*start));
                    self.s("..");
                    self.s(ast.text(*end));
                    if let Some(s) = step {
                        self.s("..");
                        self.s(ast.text(*s));
                    }
                    self.s(")");
                }
            },
            Part::ExtGlob { op, alts } => {
                self.out.push(*op as char);
                self.s("(");
                for (i, a) in alts.iter().enumerate() {
                    if i > 0 {
                        self.s("|");
                    }
                    self.word(a);
                }
                self.s(")");
            }
            Part::Array(arr) => self.array(arr),
        }
    }

    fn param(&mut self, p: &Param) {
        let ast = self.ast;
        self.s(if p.braced { "${" } else { "$" });
        match p.prefix {
            ParamPrefix::None => {}
            ParamPrefix::Length => self.s("#"),
            ParamPrefix::Indirect => self.s("!"),
        }
        self.s(ast.text(p.name_span));
        match &p.index {
            None => {}
            Some(Subscript::At) => self.s("[@]"),
            Some(Subscript::Star) => self.s("[*]"),
            Some(Subscript::Expr(w)) => {
                self.s("[");
                self.word(w);
                self.s("]");
            }
        }
        match &p.op {
            ParamOp::None => {}
            ParamOp::Default { kind, colon, word } => {
                self.s(" ");
                if *colon {
                    self.s(":");
                }
                self.s(match kind {
                    DefaultKind::Use => "-",
                    DefaultKind::Assign => "=",
                    DefaultKind::Error => "?",
                    DefaultKind::Alternative => "+",
                });
                self.word(word);
            }
            ParamOp::RemovePrefix { longest, pattern } => {
                self.s(if *longest { " ##" } else { " #" });
                self.word(pattern);
            }
            ParamOp::RemoveSuffix { longest, pattern } => {
                self.s(if *longest { " %%" } else { " %" });
                self.word(pattern);
            }
            ParamOp::Replace {
                kind,
                pattern,
                replacement,
            } => {
                self.s(match kind {
                    ReplaceKind::First => " /",
                    ReplaceKind::All => " //",
                    ReplaceKind::Prefix => " /#",
                    ReplaceKind::Suffix => " /%",
                });
                self.word(pattern);
                if let Some(r) = replacement {
                    self.s("/");
                    self.word(r);
                }
            }
            ParamOp::Case { kind, pattern } => {
                self.s(match kind {
                    CaseKind::UpperFirst => " ^",
                    CaseKind::UpperAll => " ^^",
                    CaseKind::LowerFirst => " ,",
                    CaseKind::LowerAll => " ,,",
                    CaseKind::ToggleFirst => " ~",
                    CaseKind::ToggleAll => " ~~",
                });
                if let Some(p) = pattern {
                    self.word(p);
                }
            }
            ParamOp::Substring { offset, length } => {
                self.s(" :");
                self.word(offset);
                if let Some(l) = length {
                    self.s(":");
                    self.word(l);
                }
            }
            ParamOp::Transform(c) => {
                self.s(" @");
                self.out.push(*c as char);
            }
            ParamOp::Names { star } => self.s(if *star { "*" } else { "@" }),
        }
        if p.braced {
            self.s("}");
        }
    }
}
