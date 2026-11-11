//! Variables with a literal value known before they are used (D1240).
//!
//! `S=/x; sed -n 1,2p $S/f` reads only if `$S` is `/x` there. A variable is known when
//! every write to it anywhere in the script (assignments, declaration builtins, `read`,
//! `for`, `unset`, arithmetic, `${S:=…}`, ...) is the same literal, and when, at the use,
//! an assignment of that literal has definitely run: it is an earlier command of an
//! enclosing list (or an earlier `&&` step of the same chain), run in the current shell.
//! The first half is this scan; the walker keeps the second (`Walker::defined`). Anything
//! that can assign variables the scan cannot see (`eval`, `source`, `trap`, aliases,
//! namerefs, dynamic command names or variable names) turns tracking off for the script.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use psh_syntax::{
    AssignValue, Ast, Brace, CmdId, Command, CommandKind, CondExpr, CondKind, DefaultKind, ListId,
    Param, ParamName, ParamOp, Part, RedirTarget, Subscript, Word, WordPart, backtick_body,
    literal_text,
};

/// Variables the shell itself sets or that change how other things work: never tracked.
fn special(name: &str) -> bool {
    matches!(
        name,
        "PWD"
            | "OLDPWD"
            | "RANDOM"
            | "SRANDOM"
            | "SECONDS"
            | "LINENO"
            | "EPOCHSECONDS"
            | "EPOCHREALTIME"
            | "BASHPID"
            | "FUNCNAME"
            | "REPLY"
            | "OPTARG"
            | "OPTIND"
            | "IFS"
            | "HISTCMD"
            | "COLUMNS"
            | "LINES"
            | "PPID"
            | "UID"
            | "EUID"
            | "SHLVL"
            | "_"
            | "PIPESTATUS"
            | "GROUPS"
            | "DIRSTACK"
            | "COMP_WORDS"
    ) || name.starts_with("BASH")
        || crate::dangerous_var(name)
}

#[derive(Default)]
pub(crate) struct Vars {
    /// Per name: `Some(value)` while every write is that literal; `None` once any write is
    /// something else.
    writes: BTreeMap<String, Option<String>>,
    disabled: bool,
}

impl Vars {
    pub(crate) fn scan(ast: &Ast) -> Vars {
        let mut v = Vars::default();
        v.list(ast, ast.root());
        v
    }

    /// The literal value of a variable whose every write is that literal.
    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        if self.disabled {
            return None;
        }
        self.writes.get(name)?.as_deref()
    }

    /// The tracked variables a command definitely assigns when it runs in the current
    /// shell: a bare assignment (`S=/x`) or a declaration builtin (`export S=/x`).
    pub(crate) fn assigned(&self, ast: &Ast, id: CmdId) -> Vec<String> {
        let mut out = Vec::new();
        let cmd = ast.cmd(id);
        let CommandKind::Simple(sc) = &cmd.kind else {
            return out;
        };
        if !cmd.redirects.is_empty() {
            return out;
        }
        if sc.words.is_empty() {
            for a in &sc.assigns {
                let name = ast.text(a.name);
                if self.value(name).is_some() {
                    out.push(String::from(name));
                }
            }
        } else if literal_text(ast, &sc.words[0]).is_some_and(|n| {
            matches!(
                n.as_str(),
                "export" | "readonly" | "declare" | "typeset" | "local"
            )
        }) {
            for w in &sc.words[1..] {
                if let Some((name, _)) = literal_text(ast, w)
                    .as_deref()
                    .and_then(|t| t.split_once('='))
                    && self.value(name).is_some()
                {
                    out.push(String::from(name));
                }
            }
        }
        out
    }

    fn poison(&mut self, name: &str) {
        self.writes.insert(String::from(name), None);
    }

    fn write(&mut self, name: &str, value: Option<String>) {
        let value = value.filter(|v| !v.contains('~') && !special(name));
        match (self.writes.get(name), value) {
            (None, Some(v)) => {
                self.writes.insert(String::from(name), Some(v));
            }
            (Some(Some(old)), Some(v)) if *old == v => {}
            _ => self.poison(name),
        }
    }

    /// Every identifier in arithmetic text may be assigned (`$((i++))`, `a[n=1]`).
    fn arith(&mut self, ast: &Ast, w: &Word) {
        let text = ast.text(w.span);
        let mut cur = String::new();
        for c in text.chars().chain(core::iter::once(' ')) {
            if c.is_ascii_alphanumeric() || c == '_' {
                cur.push(c);
            } else {
                if cur.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
                    self.poison(&cur);
                }
                cur.clear();
            }
        }
        self.word(ast, w);
    }

    fn list(&mut self, ast: &Ast, id: ListId) {
        for item in &ast.list(id).items {
            let pipes = core::iter::once(&item.first).chain(item.rest.iter().map(|(_, p)| p));
            for p in pipes {
                for s in &p.stages {
                    self.command(ast, s.cmd);
                }
            }
        }
    }

    fn command(&mut self, ast: &Ast, id: CmdId) {
        let cmd: &Command = ast.cmd(id);
        match &cmd.kind {
            CommandKind::Simple(sc) => {
                for a in &sc.assigns {
                    let name = ast.text(a.name);
                    if let Some(i) = &a.index {
                        self.arith(ast, i);
                    }
                    match &a.value {
                        AssignValue::Scalar(w) if a.index.is_none() && !a.append => {
                            self.write(name, literal_text(ast, w));
                            self.word(ast, w);
                        }
                        AssignValue::Scalar(w) => {
                            self.poison(name);
                            self.word(ast, w);
                        }
                        AssignValue::Array(arr) => {
                            self.poison(name);
                            for e in &arr.elems {
                                if let Some(k) = &e.key {
                                    self.arith(ast, k);
                                }
                                self.word(ast, &e.value);
                            }
                        }
                    }
                }
                for w in &sc.words {
                    self.word(ast, w);
                }
                if let Some(first) = sc.words.first() {
                    match literal_text(ast, first) {
                        Some(name) => self.builtin(ast, &name, &sc.words[1..]),
                        None => self.disabled = true,
                    }
                }
            }
            CommandKind::Brace(l) | CommandKind::Subshell(l) => self.list(ast, *l),
            CommandKind::If(c) => {
                for b in &c.branches {
                    self.list(ast, b.cond);
                    self.list(ast, b.body);
                }
                if let Some(e) = c.else_body {
                    self.list(ast, e);
                }
            }
            CommandKind::While(c) => {
                self.list(ast, c.cond);
                self.list(ast, c.body);
            }
            CommandKind::For(c) | CommandKind::Select(c) => {
                match literal_text(ast, &c.var) {
                    Some(n) => self.poison(&n),
                    None => self.disabled = true,
                }
                for w in c.words.iter().flatten() {
                    self.word(ast, w);
                }
                self.list(ast, c.body);
            }
            CommandKind::ArithFor(c) => {
                for w in [&c.init, &c.cond, &c.step] {
                    self.arith(ast, w);
                }
                self.list(ast, c.body);
            }
            CommandKind::Case(c) => {
                self.word(ast, &c.word);
                for item in &c.items {
                    for p in &item.patterns {
                        self.word(ast, p);
                    }
                    if let Some(b) = item.body {
                        self.list(ast, b);
                    }
                }
            }
            CommandKind::Arith(w) => self.arith(ast, w),
            CommandKind::Cond(e) => self.cond(ast, e),
            CommandKind::Function(f) => self.command(ast, f.body),
            CommandKind::Coproc(c) => self.command(ast, c.body),
        }
        for r in &cmd.redirects {
            match &r.target {
                RedirTarget::Word(w) => self.word(ast, w),
                RedirTarget::HereDoc(h) => {
                    let doc = ast.heredoc(*h);
                    if !doc.quoted {
                        self.word(ast, &doc.body);
                    }
                }
            }
        }
    }

    /// Builtins that assign variables named by their arguments.
    fn builtin(&mut self, ast: &Ast, name: &str, args: &[Word]) {
        let values: Vec<Option<String>> = args.iter().map(|w| literal_text(ast, w)).collect();
        match name {
            "eval" | "source" | "." | "trap" | "alias" | "enable" => self.disabled = true,
            // They run the builtin named next.
            "command" | "builtin" | "exec" => {
                let k = values
                    .iter()
                    .take_while(|v| v.as_deref().is_some_and(|v| v.starts_with('-')))
                    .count();
                match values.get(k) {
                    Some(Some(next)) => self.builtin(ast, next, &args[k + 1..]),
                    Some(None) => self.disabled = true,
                    None => {}
                }
            }
            "unset" => {
                for v in &values {
                    match v.as_deref() {
                        Some(v) if v.starts_with('-') => {}
                        Some(v) => self.poison(v),
                        None => self.disabled = true,
                    }
                }
            }
            "read" | "mapfile" | "readarray" => {
                // Options whose value is not a variable name.
                let valued = if name == "read" { "dintpu" } else { "dnOsuCc" };
                let mut skip = false;
                for v in &values {
                    if skip {
                        skip = false;
                        continue;
                    }
                    match v.as_deref() {
                        Some(o) if o.starts_with('-') && o.len() > 1 => {
                            let last = o.chars().last().unwrap_or('-');
                            skip = valued.contains(last);
                            // `read -a NAME`: the next word is a name; poisoned below.
                        }
                        Some(n) => self.poison(n),
                        None => self.disabled = true,
                    }
                }
            }
            "getopts" => match values.get(1) {
                Some(Some(n)) => self.poison(n),
                Some(None) => self.disabled = true,
                None => {}
            },
            "wait" => {
                if let Some(k) = values.iter().position(|v| v.as_deref() == Some("-p")) {
                    match values.get(k + 1) {
                        Some(Some(n)) => self.poison(n),
                        _ => self.disabled = true,
                    }
                }
            }
            "printf" => {
                for (k, v) in values.iter().enumerate() {
                    if let Some(v) = v.as_deref()
                        && let Some(rest) = v.strip_prefix("-v")
                    {
                        match (rest.is_empty(), values.get(k + 1)) {
                            (false, _) => self.poison(rest),
                            (true, Some(Some(n))) => self.poison(n),
                            _ => self.disabled = true,
                        }
                    }
                }
            }
            "let" => {
                for w in args {
                    self.arith(ast, w);
                }
            }
            "export" | "readonly" | "declare" | "typeset" | "local" => {
                let mut attrs = false;
                for (w, v) in args.iter().zip(&values) {
                    let Some(v) = v else {
                        // `declare a=(…)` or `export X=$y`: a name we can still read.
                        let text = ast.text(w.span);
                        match text.split_once('=') {
                            Some((n, _)) if is_name(n.trim_end_matches('+')) => {
                                self.poison(n.trim_end_matches('+'))
                            }
                            _ => self.disabled = true,
                        }
                        continue;
                    };
                    if let Some(opts) = v.strip_prefix('-').or_else(|| v.strip_prefix('+')) {
                        if opts.contains('n') {
                            // A nameref aliases another variable.
                            self.disabled = true;
                        }
                        if opts
                            .chars()
                            .any(|c| !matches!(c, 'x' | 'g' | 'r' | 'f' | 'F' | 'p'))
                        {
                            attrs = true;
                        }
                        continue;
                    }
                    match v.split_once('=') {
                        Some((n, val)) if !attrs && !n.ends_with('+') => {
                            self.write(n, Some(String::from(val)))
                        }
                        Some((n, _)) => self.poison(n.trim_end_matches('+')),
                        // `local x`, `declare x` reset it in a function.
                        None if matches!(name, "local" | "declare" | "typeset") || attrs => {
                            self.poison(v)
                        }
                        None => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn cond(&mut self, ast: &Ast, e: &CondExpr) {
        match &e.kind {
            CondKind::Unary { arg, .. } => self.word(ast, arg),
            CondKind::Binary { lhs, rhs, .. } => {
                self.word(ast, lhs);
                self.word(ast, rhs);
            }
            CondKind::Word(w) => self.word(ast, w),
            CondKind::Not(x) | CondKind::Group(x) => self.cond(ast, x),
            CondKind::And(a, b) | CondKind::Or(a, b) => {
                self.cond(ast, a);
                self.cond(ast, b);
            }
        }
    }

    fn word(&mut self, ast: &Ast, w: &Word) {
        self.parts(ast, &w.parts, false);
    }

    fn parts(&mut self, ast: &Ast, parts: &[WordPart], in_dq: bool) {
        for p in parts {
            match &p.kind {
                Part::Double(inner) | Part::Locale(inner) => self.parts(ast, inner, true),
                Part::Param(param) => self.param(ast, param),
                Part::CmdSubst(l)
                | Part::FunSub { list: l, .. }
                | Part::ProcSubst { list: l, .. } => self.list(ast, *l),
                Part::Backtick => {
                    match psh_syntax::parse(&backtick_body(ast.text(p.span), in_dq)) {
                        Ok(inner) => self.list(&inner, inner.root()),
                        Err(_) => self.disabled = true,
                    }
                }
                Part::Arith(w) => self.arith(ast, w),
                Part::Brace(b) => {
                    if let Brace::Alts(ws) = &**b {
                        for w in ws {
                            self.parts(ast, &w.parts, in_dq);
                        }
                    }
                }
                Part::ExtGlob { alts, .. } => {
                    for w in alts {
                        self.word(ast, w);
                    }
                }
                Part::Array(arr) => {
                    for e in &arr.elems {
                        if let Some(k) = &e.key {
                            self.arith(ast, k);
                        }
                        self.word(ast, &e.value);
                    }
                }
                Part::Lit | Part::Escaped | Part::Single | Part::AnsiC | Part::BadSubst => {}
            }
        }
    }

    fn param(&mut self, ast: &Ast, p: &Param) {
        if p.prefix == psh_syntax::ParamPrefix::Indirect {
            // `${!x}` reads a variable named by another; it assigns nothing, but with
            // `${!x:=v}` it would.
            if !matches!(p.op, ParamOp::None) {
                self.disabled = true;
            }
        }
        if let Some(Subscript::Expr(w)) = &p.index {
            self.arith(ast, w);
        }
        match &p.op {
            ParamOp::Default { kind, word, .. } => {
                if *kind == DefaultKind::Assign && p.name == ParamName::Var {
                    self.poison(ast.text(p.name_span));
                }
                self.word(ast, word);
            }
            ParamOp::RemovePrefix { pattern, .. } | ParamOp::RemoveSuffix { pattern, .. } => {
                self.word(ast, pattern)
            }
            ParamOp::Replace {
                pattern,
                replacement,
                ..
            } => {
                self.word(ast, pattern);
                if let Some(r) = replacement {
                    self.word(ast, r);
                }
            }
            ParamOp::Case { pattern, .. } => {
                if let Some(pat) = pattern {
                    self.word(ast, pat);
                }
            }
            ParamOp::Substring { offset, length } => {
                self.arith(ast, offset);
                if let Some(l) = length {
                    self.arith(ast, l);
                }
            }
            ParamOp::None | ParamOp::Transform(_) | ParamOp::Names { .. } => {}
        }
    }
}

fn is_name(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The value of a word after quote removal, with known variables substituted; `None` if
/// anything else is dynamic. An unquoted variable whose value would be split or globbed
/// is not substituted.
pub(crate) fn value(
    ast: &Ast,
    w: &Word,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let mut out = String::new();
    collect(ast, &w.parts, false, lookup, &mut out).then_some(out)
}

fn collect(
    ast: &Ast,
    parts: &[WordPart],
    in_dq: bool,
    lookup: &dyn Fn(&str) -> Option<String>,
    out: &mut String,
) -> bool {
    for p in parts {
        let text = ast.text(p.span);
        match &p.kind {
            Part::Lit => out.push_str(text),
            Part::Escaped => out.push_str(&text[1..]),
            Part::Single => out.push_str(&text[1..text.len() - 1]),
            Part::Double(inner) | Part::Locale(inner) => {
                if !collect(ast, inner, true, lookup, out) {
                    return false;
                }
            }
            Part::Param(param)
                if param.name == ParamName::Var
                    && param.prefix == psh_syntax::ParamPrefix::None
                    && param.index.is_none()
                    && matches!(param.op, ParamOp::None) =>
            {
                let Some(v) = lookup(ast.text(param.name_span)) else {
                    return false;
                };
                if !in_dq && (v.is_empty() || v.contains([' ', '\t', '\n', '*', '?', '['])) {
                    return false;
                }
                out.push_str(&v);
            }
            _ => return false,
        }
    }
    true
}
