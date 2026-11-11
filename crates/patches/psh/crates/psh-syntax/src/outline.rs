//! A readable outline of the tree, for people: one row per node worth looking at, in
//! source order, each with its depth, kind and source text. The docs' AST tab and
//! `psh --parse --tree` show it; tools that need every field use the JSON of
//! `psh --parse`, which this leaves out on purpose.
//!
//! What it drops: wrappers with one child (a list of one and-or list, an and-or list of
//! one pipeline, a pipeline of one command are shown as that child), the parts of words
//! without expansions (one `word` row), a word of one part (shown as the part), and
//! fields at their defaults. Kinds are the `type` names of `psh --parse`. A row's role
//! names its place in the parent (`then`, `do`, `pattern`, ...) where the kind alone
//! does not.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::ast::*;
use crate::span::Span;
use crate::words::{backtick_body, literal_text};

/// One node of [`outline`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineRow {
    /// Nesting depth; children follow their parent with `depth + 1`.
    pub depth: u32,
    /// Its place in the parent (`do`, `then`, `pattern`, `index`, ...), or `""`.
    pub role: &'static str,
    /// The node type, as `psh --parse` names it (`simple`, `pipeline`, `param`, ...).
    pub kind: &'static str,
    /// What the source text does not show at a glance: operators, names, flags.
    pub info: String,
    /// The node's source text, its first line, cut at [`TEXT_CHARS`] chars (with `…`
    /// when cut).
    pub text: String,
    /// Byte span in the script. Rows inside a backquoted body, which is parsed on its
    /// own, have the span of the whole `` `…` ``.
    pub span: Span,
}

/// How much of a node's source a row keeps.
pub const TEXT_CHARS: usize = 60;

/// The outline of `ast`, in source order.
pub fn outline(ast: &Ast) -> Vec<OutlineRow> {
    let mut o = Outliner {
        ast,
        rows: Vec::new(),
        depth: 0,
        in_double: false,
        fixed: None,
    };
    o.list(ast.root(), "");
    o.rows
}

/// The outline as indented text, one row per line: role, kind and info, then the
/// source text in a column of its own.
pub fn outline_text(rows: &[OutlineRow]) -> String {
    let labels: Vec<String> = rows.iter().map(label).collect();
    let width = labels
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .min(40);
    let mut out = String::new();
    for (row, l) in rows.iter().zip(&labels) {
        if row.text.is_empty() {
            out.push_str(l);
        } else {
            let _ = write!(out, "{l:width$}  {}", row.text);
        }
        out.push('\n');
    }
    out
}

fn label(row: &OutlineRow) -> String {
    let mut s = String::new();
    for _ in 0..row.depth {
        s.push_str("  ");
    }
    if !row.role.is_empty() {
        s.push_str(row.role);
        s.push_str(": ");
    }
    s.push_str(row.kind);
    if !row.info.is_empty() {
        s.push(' ');
        s.push_str(&row.info);
    }
    s
}

/// The first line of `text`, at most [`TEXT_CHARS`] chars.
fn excerpt(text: &str) -> String {
    // A here-document body ends with its newline.
    let text = text.strip_suffix('\n').unwrap_or(text);
    let line = text.split('\n').next().unwrap_or("");
    let mut out: String = line.chars().take(TEXT_CHARS).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

/// `items` joined by spaces, skipping empty ones.
fn join(items: &[&str]) -> String {
    let mut s = String::new();
    for i in items.iter().filter(|i| !i.is_empty()) {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(i);
    }
    s
}

struct Outliner<'a> {
    ast: &'a Ast,
    rows: Vec<OutlineRow>,
    depth: u32,
    /// Inside double quotes, where a backquoted body unescapes `\"` too.
    in_double: bool,
    /// The span every row gets inside a backquoted body (its spans are offsets into the
    /// unescaped body, not into the script).
    fixed: Option<Span>,
}

impl<'a> Outliner<'a> {
    fn span(&self, span: Span) -> Span {
        self.fixed.unwrap_or_else(|| self.ast.original_span(span))
    }

    fn leaf(&mut self, role: &'static str, kind: &'static str, info: String, span: Span) {
        self.rows.push(OutlineRow {
            depth: self.depth,
            role,
            kind,
            info,
            text: excerpt(self.ast.text(span)),
            span: self.span(span),
        });
    }

    fn node(
        &mut self,
        role: &'static str,
        kind: &'static str,
        info: String,
        span: Span,
        children: impl FnOnce(&mut Self),
    ) {
        self.leaf(role, kind, info, span);
        self.depth += 1;
        children(self);
        self.depth -= 1;
    }

    fn list(&mut self, id: ListId, role: &'static str) {
        let l = self.ast.list(id);
        match l.items.as_slice() {
            [one] => self.and_or(one, role),
            items if role.is_empty() => {
                for ao in items {
                    self.and_or(ao, "");
                }
            }
            items => self.node(role, "list", String::new(), l.span, |o| {
                for ao in items {
                    o.and_or(ao, "");
                }
            }),
        }
    }

    fn and_or(&mut self, ao: &'a AndOr, role: &'static str) {
        if ao.rest.is_empty() && !ao.background {
            return self.pipeline(&ao.first, role);
        }
        let mut ops: Vec<&str> = ao
            .rest
            .iter()
            .map(|(op, _)| match op {
                AndOrOp::And => "&&",
                AndOrOp::Or => "||",
            })
            .collect();
        if ao.background {
            ops.push("&");
        }
        self.node(role, "and_or", join(&ops), ao.span, |o| {
            o.pipeline(&ao.first, "");
            for (_, p) in &ao.rest {
                o.pipeline(p, "");
            }
        });
    }

    fn pipeline(&mut self, p: &'a Pipeline, role: &'static str) {
        if let ([one], 0, None) = (p.stages.as_slice(), p.bangs, p.time) {
            return self.cmd(one.cmd, role);
        }
        let mut info: Vec<&str> = Vec::new();
        if let Some(t) = p.time {
            info.push(if t.posix { "time -p" } else { "time" });
        }
        info.extend(core::iter::repeat_n("!", p.bangs as usize));
        for s in &p.stages[..p.stages.len().saturating_sub(1)] {
            info.push(if s.pipe_stderr { "|&" } else { "|" });
        }
        self.node(role, "pipeline", join(&info), p.span, |o| {
            for s in &p.stages {
                o.cmd(s.cmd, "");
            }
        });
    }

    fn cmd(&mut self, id: CmdId, role: &'static str) {
        let ast = self.ast;
        let c = ast.cmd(id);
        let (kind, info) = match &c.kind {
            CommandKind::Simple(_) => ("simple", String::new()),
            CommandKind::Brace(_) => ("brace", String::new()),
            CommandKind::Subshell(_) => ("subshell", String::new()),
            CommandKind::If(_) => ("if", String::new()),
            CommandKind::While(w) => (if w.until { "until" } else { "while" }, String::new()),
            CommandKind::For(f) => ("for", ast.text(f.var.span).into()),
            CommandKind::Select(f) => ("select", ast.text(f.var.span).into()),
            CommandKind::ArithFor(_) => ("arith_for", String::new()),
            CommandKind::Case(_) => ("case", String::new()),
            CommandKind::Arith(_) => ("arith", String::new()),
            CommandKind::Cond(_) => ("cond", String::new()),
            CommandKind::Function(f) => ("function", ast.text(f.name.span).into()),
            CommandKind::Coproc(cp) => (
                "coproc",
                cp.name.as_ref().map_or("", |n| ast.text(n.span)).into(),
            ),
        };
        self.node(role, kind, info, c.span, |o| {
            o.cmd_children(&c.kind);
            for r in &c.redirects {
                o.redirect(r);
            }
        });
    }

    fn cmd_children(&mut self, kind: &'a CommandKind) {
        match kind {
            CommandKind::Simple(s) => {
                for a in &s.assigns {
                    self.assign(a);
                }
                for w in &s.words {
                    self.word(w, "");
                }
            }
            CommandKind::Brace(l) | CommandKind::Subshell(l) => self.list(*l, ""),
            CommandKind::If(i) => {
                for (n, b) in i.branches.iter().enumerate() {
                    self.list(b.cond, if n == 0 { "if" } else { "elif" });
                    self.list(b.body, "then");
                }
                if let Some(e) = i.else_body {
                    self.list(e, "else");
                }
            }
            CommandKind::While(w) => {
                self.list(w.cond, if w.until { "until" } else { "while" });
                self.list(w.body, "do");
            }
            CommandKind::For(f) | CommandKind::Select(f) => {
                for w in f.words.iter().flatten() {
                    self.word(w, "in");
                }
                self.list(f.body, "do");
            }
            CommandKind::ArithFor(f) => {
                self.word(&f.init, "init");
                self.word(&f.cond, "cond");
                self.word(&f.step, "step");
                self.list(f.body, "do");
            }
            CommandKind::Case(cs) => {
                self.word(&cs.word, "word");
                for item in &cs.items {
                    let term = match item.terminator {
                        CaseTerminator::Break => ";;",
                        CaseTerminator::FallThrough => ";&",
                        CaseTerminator::Continue => ";;&",
                    };
                    self.node("", "item", term.into(), item.span, |o| {
                        for p in &item.patterns {
                            o.word(p, "pattern");
                        }
                        if let Some(b) = item.body {
                            o.list(b, "body");
                        }
                    });
                }
            }
            CommandKind::Arith(w) => self.word(w, "expr"),
            CommandKind::Cond(e) => self.cond(e, ""),
            CommandKind::Function(f) => self.cmd(f.body, "body"),
            CommandKind::Coproc(cp) => self.cmd(cp.body, "body"),
        }
    }

    fn redirect(&mut self, r: &'a Redirect) {
        let mut info = match r.fd {
            Some(RedirFd::Number(n)) => alloc::format!("{n}"),
            Some(RedirFd::Var(s)) => alloc::format!("{{{}}}", self.ast.text(s)),
            None => String::new(),
        };
        info.push_str(r.op.as_str());
        let heredoc = match &r.target {
            RedirTarget::Word(_) => None,
            RedirTarget::HereDoc(id) => Some(self.ast.heredoc(*id)),
        };
        if heredoc.is_some_and(|h| !h.terminated) {
            info.push_str(" unterminated");
        }
        self.node("", "redirect", info, r.span, |o| {
            match (&r.target, heredoc) {
                (RedirTarget::Word(w), _) => o.word(w, ""),
                (_, Some(h)) => {
                    o.word(&h.delimiter, "delimiter");
                    o.word(&h.body, "body");
                }
                _ => {}
            }
        });
    }

    fn assign(&mut self, a: &'a Assign) {
        let mut info = String::from(self.ast.text(a.name));
        if a.index.is_some() {
            info.push_str("[…]");
        }
        info.push_str(if a.append { "+=" } else { "=" });
        self.node("", "assign", info, a.span, |o| {
            if let Some(i) = &a.index {
                o.word(i, "index");
            }
            match &a.value {
                AssignValue::Scalar(w) => o.word(w, ""),
                AssignValue::Array(arr) => o.array(arr, ""),
            }
        });
    }

    fn array(&mut self, arr: &'a ArrayLit, role: &'static str) {
        self.node(role, "array", String::new(), arr.span, |o| {
            for e in &arr.elems {
                match &e.key {
                    None if !e.append => o.word(&e.value, ""),
                    key => {
                        let op = if e.append { "+=" } else { "=" };
                        o.node("", "elem", op.into(), e.span, |o| {
                            if let Some(k) = key {
                                o.word(k, "key");
                            }
                            o.word(&e.value, "");
                        });
                    }
                }
            }
        });
    }

    fn cond(&mut self, e: &'a CondExpr, role: &'static str) {
        match &e.kind {
            CondKind::Unary { op_span, arg, .. } => {
                let op = self.ast.text(*op_span).into();
                self.node(role, "cond_unary", op, e.span, |o| o.word(arg, ""));
            }
            CondKind::Binary {
                op_span, lhs, rhs, ..
            } => {
                let op = self.ast.text(*op_span).into();
                self.node(role, "cond_binary", op, e.span, |o| {
                    o.word(lhs, "");
                    o.word(rhs, "");
                });
            }
            CondKind::Word(w) => self.word(w, role),
            CondKind::Not(i) => {
                self.node(role, "cond_not", "!".into(), e.span, |o| o.cond(i, ""));
            }
            CondKind::Group(i) => {
                self.node(role, "cond_group", String::new(), e.span, |o| o.cond(i, ""));
            }
            CondKind::And(a, b) | CondKind::Or(a, b) => {
                let (kind, op) = match e.kind {
                    CondKind::And(..) => ("cond_and", "&&"),
                    _ => ("cond_or", "||"),
                };
                self.node(role, kind, op.into(), e.span, |o| {
                    o.cond(a, "");
                    o.cond(b, "");
                });
            }
        }
    }

    fn word(&mut self, w: &'a Word, role: &'static str) {
        if literal_text(self.ast, w).is_some() {
            return self.leaf(role, "word", String::new(), w.span);
        }
        match w.parts.as_slice() {
            [one] => self.part(one, role),
            parts => self.node(role, "word", String::new(), w.span, |o| {
                for p in parts {
                    o.part(p, "");
                }
            }),
        }
    }

    fn part(&mut self, p: &'a WordPart, role: &'static str) {
        let sp = p.span;
        let leaf = |o: &mut Self, kind| o.leaf(role, kind, String::new(), sp);
        match &p.kind {
            Part::Lit => leaf(self, "lit"),
            Part::Escaped => leaf(self, "escaped"),
            Part::Single => leaf(self, "single"),
            Part::AnsiC => leaf(self, "ansi_c"),
            Part::BadSubst => leaf(self, "bad_subst"),
            Part::Double(inner) | Part::Locale(inner) => {
                let kind = if matches!(p.kind, Part::Double(_)) {
                    "double"
                } else {
                    "locale"
                };
                self.node(role, kind, String::new(), sp, |o| {
                    let outer = core::mem::replace(&mut o.in_double, true);
                    for q in inner {
                        o.part(q, "");
                    }
                    o.in_double = outer;
                });
            }
            Part::Param(pe) => self.param(pe, sp, role),
            Part::CmdSubst(l) => self.subst(role, "cmd_subst", "", sp, *l),
            Part::FunSub { list, reply } => {
                self.subst(role, "funsub", if *reply { "|" } else { "" }, sp, *list);
            }
            Part::ProcSubst { output, list } => {
                self.subst(
                    role,
                    "proc_subst",
                    if *output { ">" } else { "<" },
                    sp,
                    *list,
                );
            }
            Part::Backtick => {
                let inner = backtick_body(self.ast.text(sp), self.in_double);
                let fixed = self.span(sp);
                self.node(
                    role,
                    "backtick",
                    String::new(),
                    sp,
                    |o| match crate::parse(&inner) {
                        Ok(ast) => {
                            let mut sub = Outliner {
                                ast: &ast,
                                rows: Vec::new(),
                                depth: o.depth,
                                in_double: false,
                                fixed: Some(fixed),
                            };
                            sub.list(ast.root(), "");
                            o.rows.append(&mut sub.rows);
                        }
                        Err(e) => o.leaf("", "error", e.message, sp),
                    },
                );
            }
            Part::Arith(w) => self.node(role, "arith", String::new(), sp, |o| o.word(w, "expr")),
            Part::Brace(b) => match &**b {
                Brace::Alts(alts) => self.node(role, "brace", String::new(), sp, |o| {
                    for w in alts {
                        o.word(w, "");
                    }
                }),
                Brace::Seq { .. } => leaf(self, "brace_seq"),
            },
            Part::ExtGlob { op, alts } => {
                let op = char::from(*op).into();
                self.node(role, "extglob", op, sp, |o| {
                    for w in alts {
                        o.word(w, "");
                    }
                });
            }
            Part::Array(arr) => self.array(arr, role),
        }
    }

    fn subst(&mut self, role: &'static str, kind: &'static str, info: &str, sp: Span, l: ListId) {
        self.node(role, kind, info.into(), sp, |o| {
            let outer = core::mem::replace(&mut o.in_double, false);
            o.list(l, "");
            o.in_double = outer;
        });
    }

    fn param(&mut self, pe: &'a Param, sp: Span, role: &'static str) {
        let mut name = String::from(match pe.prefix {
            ParamPrefix::None => "",
            ParamPrefix::Length => "#",
            ParamPrefix::Indirect => "!",
        });
        name.push_str(self.ast.text(pe.name_span));
        match &pe.index {
            None => {}
            Some(Subscript::At) => name.push_str("[@]"),
            Some(Subscript::Star) => name.push_str("[*]"),
            Some(Subscript::Expr(_)) => name.push_str("[…]"),
        }
        let transform;
        let op = match &pe.op {
            ParamOp::None => "",
            ParamOp::Default { kind, colon, .. } => match (kind, colon) {
                (DefaultKind::Use, false) => "-",
                (DefaultKind::Use, true) => ":-",
                (DefaultKind::Assign, false) => "=",
                (DefaultKind::Assign, true) => ":=",
                (DefaultKind::Error, false) => "?",
                (DefaultKind::Error, true) => ":?",
                (DefaultKind::Alternative, false) => "+",
                (DefaultKind::Alternative, true) => ":+",
            },
            ParamOp::RemovePrefix { longest, .. } => ["#", "##"][usize::from(*longest)],
            ParamOp::RemoveSuffix { longest, .. } => ["%", "%%"][usize::from(*longest)],
            ParamOp::Replace { kind, .. } => match kind {
                ReplaceKind::First => "/",
                ReplaceKind::All => "//",
                ReplaceKind::Prefix => "/#",
                ReplaceKind::Suffix => "/%",
            },
            ParamOp::Case { kind, .. } => match kind {
                CaseKind::UpperFirst => "^",
                CaseKind::UpperAll => "^^",
                CaseKind::LowerFirst => ",",
                CaseKind::LowerAll => ",,",
                CaseKind::ToggleFirst => "~",
                CaseKind::ToggleAll => "~~",
            },
            ParamOp::Substring { .. } => ":",
            ParamOp::Transform(c) => {
                transform = alloc::format!("@{}", char::from(*c));
                &transform
            }
            ParamOp::Names { star } => ["names @", "names *"][usize::from(*star)],
        };
        self.node(role, "param", join(&[&name, op]), sp, |o| {
            if let Some(Subscript::Expr(w)) = &pe.index {
                o.word(w, "index");
            }
            match &pe.op {
                ParamOp::Default { word, .. } => o.word(word, "word"),
                ParamOp::RemovePrefix { pattern, .. } | ParamOp::RemoveSuffix { pattern, .. } => {
                    o.word(pattern, "pattern");
                }
                ParamOp::Replace {
                    pattern,
                    replacement,
                    ..
                } => {
                    o.word(pattern, "pattern");
                    if let Some(r) = replacement {
                        o.word(r, "with");
                    }
                }
                ParamOp::Case {
                    pattern: Some(p), ..
                } => o.word(p, "pattern"),
                ParamOp::Substring { offset, length } => {
                    o.word(offset, "offset");
                    if let Some(l) = length {
                        o.word(l, "length");
                    }
                }
                _ => {}
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use super::*;
    use crate::{Options, parse, parse_with};

    fn t(src: &str) -> String {
        outline_text(&outline(
            &parse(src).unwrap_or_else(|e| panic!("{src:?}: {e}")),
        ))
    }

    #[test]
    fn wrappers_of_one_child_collapse() {
        assert_eq!(t("ls"), "simple  ls\n  word  ls\n");
        assert_eq!(t(""), "");
        // Several commands at the top: no list row.
        assert_eq!(t("a; b"), "simple  a\n  word  a\nsimple  b\n  word  b\n");
    }

    #[test]
    fn loop_with_pipeline() {
        let src =
            "for f in *.txt; do grep -q foo \"$f\" && echo \"$f: ${f%.txt}\"; done | sort > out";
        assert_eq!(
            t(src),
            concat!(
                "pipeline |                 for f in *.txt; do grep -q foo \"$f\" && echo \"$f: ${f%.txt}\";…\n",
                "  for f                    for f in *.txt; do grep -q foo \"$f\" && echo \"$f: ${f%.txt}\";…\n",
                "    in: word               *.txt\n",
                "    do: and_or &&          grep -q foo \"$f\" && echo \"$f: ${f%.txt}\"\n",
                "      simple               grep -q foo \"$f\"\n",
                "        word               grep\n",
                "        word               -q\n",
                "        word               foo\n",
                "        double             \"$f\"\n",
                "          param f          $f\n",
                "      simple               echo \"$f: ${f%.txt}\"\n",
                "        word               echo\n",
                "        double             \"$f: ${f%.txt}\"\n",
                "          param f          $f\n",
                "          lit              : \n",
                "          param f %        ${f%.txt}\n",
                "            pattern: word  .txt\n",
                "  simple                   sort > out\n",
                "    word                   sort\n",
                "    redirect >             > out\n",
                "      word                 out\n",
            )
        );
    }

    #[test]
    fn spans_point_into_the_script() {
        let src = "echo \"$x\" `pwd` # c";
        let rows = outline(&parse(src).unwrap());
        let texts: Vec<_> = rows.iter().map(|r| &src[r.span.range()]).collect();
        // The rows of a backquoted body carry the span of the whole `…`.
        assert_eq!(
            texts,
            [
                "echo \"$x\" `pwd`",
                "echo",
                "\"$x\"",
                "$x",
                "`pwd`",
                "`pwd`",
                "`pwd`"
            ]
        );
        let kinds: Vec<_> = rows.iter().map(|r| (r.depth, r.kind)).collect();
        assert_eq!(
            kinds,
            [
                (0, "simple"),
                (1, "word"),
                (1, "double"),
                (2, "param"),
                (1, "backtick"),
                (2, "simple"),
                (3, "word")
            ]
        );
    }

    #[test]
    fn compound_commands() {
        assert_eq!(
            t("if a; then b; c; elif d; then :; else e; fi"),
            concat!(
                "if              if a; then b; c; elif d; then :; else e; fi\n",
                "  if: simple    a\n",
                "    word        a\n",
                "  then: list    b; c\n",
                "    simple      b\n",
                "      word      b\n",
                "    simple      c\n",
                "      word      c\n",
                "  elif: simple  d\n",
                "    word        d\n",
                "  then: simple  :\n",
                "    word        :\n",
                "  else: simple  e\n",
                "    word        e\n",
            )
        );
        assert_eq!(
            t("case $x in a|b) ;; *) y ;& esac"),
            concat!(
                "case               case $x in a|b) ;; *) y ;& esac\n",
                "  word: param x    $x\n",
                "  item ;;          a|b) ;;\n",
                "    pattern: word  a\n",
                "    pattern: word  b\n",
                "  item ;&          *) y ;&\n",
                "    pattern: word  *\n",
                "    body: simple   y\n",
                "      word         y\n",
            )
        );
        let text =
            t("f() { ! time -p a |& b & }; while [[ -n $x && ( y == z ) ]]; do ((i++)); done");
        for line in [
            "function f ",
            "  body: brace ",
            "    and_or & ",
            "      pipeline time -p ! |& ",
            "  while: cond ",
            "    cond_and && ",
            "      cond_unary -n ",
            "        param x ",
            "      cond_group ",
            "        cond_binary == ",
            "  do: arith ",
            "    expr: word ",
        ] {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
    }

    #[test]
    fn words_assignments_and_redirections() {
        let text = t(
            "A[1]+=x B=(a [k]=v) cmd ${a[i]:-d} ${#v} ${x//p/r} ${x:1:2} ${x@Q} ${!p*} {a,b} {1..3} \
             $(ls) <(cat) 'q' $'c' \\e 2>&1 {fd}>f <<-'E'\n\tbody $x\n\tE\n",
        );
        for line in [
            "assign A[…]+= ",
            "  index: word ",
            "assign B= ",
            "  array ",
            "    elem = ",
            "      key: word ",
            "param a[…] :- ",
            "  index: word ",
            "  word: word ",
            "param #v ",
            "param x // ",
            "  with: word ",
            "param x : ",
            "  offset: word ",
            "  length: word ",
            "param x @Q ",
            "param !p names * ",
            "brace ",
            "brace_seq ",
            "cmd_subst ",
            "proc_subst < ",
            "word ",
            "redirect 2>& ",
            "redirect {fd}> ",
            "redirect <<- ",
            "  delimiter: word ",
            "  body: word ",
        ] {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
        let text = t("cat <<E\n$x\n");
        assert!(text.contains("redirect << unterminated"), "{text}");
        let ast = parse_with("ls @(a|b)", Options { extglob: true }).unwrap();
        let text = outline_text(&outline(&ast));
        assert!(text.contains("extglob @"), "{text}");
    }

    #[test]
    fn rows_nest_inside_their_parents() {
        // Every construct: a child is one level deeper than some row before it, and
        // its span lies inside its parent's (but for a here-document's body, which
        // follows the line of its redirection).
        let src = "A=1 b[2]+=(x [k]=v) cmd 'l' $'c' $\"$t\" \\e ${v:-w} ${#v} ${a[i]} ${x//a/b} \
                   ${x:1:2} ${x^^} ${x@Q} ${!p*} ${ f; } ${| g; } $((1+2)) {x,y} {1..2} 2>&1 {fd}<i <<'H'\n\
                   body\nH\n\
                   time ! a | b |& c && d || e &\n\
                   if i; then j; else m; fi; while n; do o; done; until p; do q; done\n\
                   for r in s; do t; done; select u; do w; done; for ((;;)); do y; done\n\
                   case z in a|b) c;; *) ;& esac; { d; }; (e); ((f)); [[ -f g && ( h == i || ! j ) && k ]]\n\
                   fn() { k; }; function g { :; }; coproc N { l; }; declare -a arr=(m) x=\"`echo \\\"y\\\"`\"";
        let rows = outline(&parse(src).unwrap());
        let mut stack: Vec<&OutlineRow> = Vec::new();
        for r in &rows {
            while stack.last().is_some_and(|p| p.depth >= r.depth) {
                stack.pop();
            }
            assert_eq!(r.depth as usize, stack.len(), "{r:?}");
            if let Some(p) = stack.last().filter(|p| p.kind != "redirect") {
                assert!(
                    p.span.start <= r.span.start && r.span.end <= p.span.end,
                    "{r:?} in {p:?}"
                );
            }
            assert!(r.span.end as usize <= src.len());
            stack.push(r);
        }
        let text = outline_text(&rows);
        for kind in [
            "funsub |",
            "locale",
            "backtick",
            "coproc N",
            "select u",
            "arith_for",
        ] {
            assert!(text.contains(kind), "{kind} in\n{text}");
        }
    }

    #[test]
    fn long_and_multiline_text_is_cut() {
        let rows = outline(&parse("echo 'a\nb'").unwrap());
        assert_eq!(rows[0].text, "echo 'a…");
        let long = "x".repeat(80);
        let rows = outline(&parse(&long).unwrap());
        assert_eq!(rows[0].text.chars().count(), TEXT_CHARS + 1);
    }
}
