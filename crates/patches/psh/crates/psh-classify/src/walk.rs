//! The walk over the AST: every simple command, with the commands nested in its words,
//! redirections and strings.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use psh_syntax::{
    AndOrOp, AssignValue, Ast, Brace, CmdId, Command, CommandKind, CondExpr, CondKind, DefaultKind,
    ListId, Param, ParamOp, Part, RedirFd, RedirOp, RedirTarget, Redirect, SimpleCommand, Span,
    Subscript, Word, WordPart, backtick_body, literal_text,
};

use crate::rules::{self, Nested, Outcome, PathRef, harmless_path};
use crate::vars::{self, Vars};
use crate::{
    Arg, Categories, Category, Classification, CommandInfo, NestedSource, PathEffect, RedirInfo,
    Via,
};

/// Nested shell strings (`bash -c "bash -c '…'"`) deeper than this are unknown.
const MAX_DEPTH: u32 = 8;

#[derive(Clone, Copy)]
struct Cx {
    source: Option<usize>,
    parent: Option<usize>,
    via: Via,
    depth: u32,
}

impl Cx {
    fn nested(self, parent: usize, via: Via) -> Cx {
        Cx {
            parent: Some(parent),
            via,
            ..self
        }
    }
}

/// What a shell would read as its script on stdin.
enum Stdin {
    /// No redirection: the terminal, the caller's stdin or a pipe.
    Inherited,
    /// A here-document or here-string without expansions.
    Literal(String, Span),
    /// A file or a here-document with expansions.
    Dynamic,
}

pub(crate) struct Walker {
    out: Classification,
    /// Categories not owned by a simple command: background jobs, redirections of
    /// compound commands.
    extra: Categories,
    functions: BTreeSet<String>,
    aliases: BTreeSet<String>,
    /// Variables with a known literal value (D1240) in the script being walked.
    vars: Vars,
    /// Tracked variables definitely assigned at this point of the walk.
    defined: Vec<String>,
}

impl Walker {
    pub(crate) fn run(ast: &Ast) -> Classification {
        let mut w = Walker {
            out: Classification::default(),
            extra: Categories::empty(),
            functions: BTreeSet::new(),
            aliases: BTreeSet::new(),
            vars: Vars::scan(ast),
            defined: Vec::new(),
        };
        let cx = Cx {
            source: None,
            parent: None,
            via: Via::Script,
            depth: 0,
        };
        w.list(ast, ast.root(), cx);
        w.finish()
    }

    fn finish(mut self) -> Classification {
        // Calls of functions and aliases defined anywhere in the script: what runs depends
        // on definitions the classifier does not follow.
        for c in &mut self.out.commands {
            if let Some(name) = &c.name {
                if self.functions.contains(name) {
                    c.categories.insert(Category::Unknown);
                    c.reason = Some("calls a function defined in the script");
                } else if self.aliases.contains(name) {
                    c.categories.insert(Category::Unknown);
                    c.reason = Some("may be an alias defined in the script");
                }
            }
        }
        let mut cats = self.extra;
        for c in &self.out.commands {
            cats.extend(c.categories);
        }
        self.out.categories = cats;
        self.out
    }

    fn list(&mut self, ast: &Ast, id: ListId, cx: Cx) {
        let mark = self.defined.len();
        for item in &ast.list(id).items {
            if item.background {
                self.extra.insert(Category::LongRunning);
            }
            // An assignment is definite for the rest of an `&&` chain, and, as the first
            // step of an item run in the current shell, for the rest of the list.
            let chain = self.defined.len();
            let mut and_only = !item.background;
            let ops = core::iter::once(None).chain(item.rest.iter().map(|(op, _)| Some(*op)));
            let pipes = core::iter::once(&item.first).chain(item.rest.iter().map(|(_, p)| p));
            for (op, p) in ops.zip(pipes) {
                if op == Some(AndOrOp::Or) {
                    and_only = false;
                    self.defined.truncate(chain);
                }
                for (k, stage) in p.stages.iter().enumerate() {
                    self.command(ast, stage.cmd, cx, k > 0);
                }
                if and_only && p.stages.len() == 1 {
                    let names = self.vars.assigned(ast, p.stages[0].cmd);
                    self.defined.extend(names);
                }
            }
            self.defined.truncate(chain);
            if !item.background && item.first.stages.len() == 1 {
                let names = self.vars.assigned(ast, item.first.stages[0].cmd);
                self.defined.extend(names);
            }
        }
        self.defined.truncate(mark);
    }

    /// A variable's value where it is used, if known (D1240).
    fn lookup(&self, name: &str) -> Option<String> {
        if self.defined.iter().any(|d| d == name) {
            self.vars.value(name).map(String::from)
        } else {
            None
        }
    }

    fn arg(&self, ast: &Ast, w: &Word) -> Arg {
        let mut a = arg(ast, w);
        if a.value.is_none() {
            a.value = vars::value(ast, w, &|n| self.lookup(n));
        }
        a
    }

    fn command(&mut self, ast: &Ast, id: CmdId, cx: Cx, piped: bool) {
        let cmd = ast.cmd(id);
        if let CommandKind::Simple(sc) = &cmd.kind {
            self.simple(ast, cmd, sc, cx, piped);
            return;
        }
        match &cmd.kind {
            CommandKind::Simple(_) => {}
            CommandKind::Brace(l) | CommandKind::Subshell(l) => self.list(ast, *l, cx),
            CommandKind::If(c) => {
                for b in &c.branches {
                    self.list(ast, b.cond, cx);
                    self.list(ast, b.body, cx);
                }
                if let Some(e) = c.else_body {
                    self.list(ast, e, cx);
                }
            }
            CommandKind::While(c) => {
                self.list(ast, c.cond, cx);
                self.list(ast, c.body, cx);
            }
            CommandKind::For(c) | CommandKind::Select(c) => {
                self.word(ast, &c.var, cx, false);
                for w in c.words.iter().flatten() {
                    self.word(ast, w, cx, false);
                }
                self.list(ast, c.body, cx);
            }
            CommandKind::ArithFor(c) => {
                for w in [&c.init, &c.cond, &c.step] {
                    self.word(ast, w, cx, false);
                }
                self.list(ast, c.body, cx);
            }
            CommandKind::Case(c) => {
                self.word(ast, &c.word, cx, false);
                for item in &c.items {
                    for p in &item.patterns {
                        self.word(ast, p, cx, false);
                    }
                    if let Some(b) = item.body {
                        self.list(ast, b, cx);
                    }
                }
            }
            CommandKind::Arith(w) => self.word(ast, w, cx, false),
            CommandKind::Cond(e) => self.cond(ast, e, cx),
            CommandKind::Function(f) => {
                if let Some(name) = literal_text(ast, &f.name) {
                    self.functions.insert(name);
                }
                self.command(ast, f.body, cx, false);
            }
            CommandKind::Coproc(c) => {
                self.extra.insert(Category::LongRunning);
                self.command(ast, c.body, cx, false);
            }
        }
        let (cats, _) = self.redirects(ast, &cmd.redirects, cx, None);
        self.extra.extend(cats);
    }

    fn cond(&mut self, ast: &Ast, e: &CondExpr, cx: Cx) {
        match &e.kind {
            CondKind::Unary { arg, .. } => self.word(ast, arg, cx, false),
            CondKind::Binary { lhs, rhs, .. } => {
                self.word(ast, lhs, cx, false);
                self.word(ast, rhs, cx, false);
            }
            CondKind::Word(w) => self.word(ast, w, cx, false),
            CondKind::Not(x) | CondKind::Group(x) => self.cond(ast, x, cx),
            CondKind::And(a, b) | CondKind::Or(a, b) => {
                self.cond(ast, a, cx);
                self.cond(ast, b, cx);
            }
        }
    }

    fn push(&mut self, info: CommandInfo) -> usize {
        self.out.commands.push(info);
        self.out.commands.len() - 1
    }

    fn mark_unknown(&mut self, idx: usize, reason: &'static str) {
        let c = &mut self.out.commands[idx];
        c.categories.insert(Category::Unknown);
        c.reason.get_or_insert(reason);
    }

    fn simple(&mut self, ast: &Ast, cmd: &Command, sc: &SimpleCommand, cx: Cx, piped: bool) {
        let words: Vec<Arg> = sc.words.iter().map(|w| self.arg(ast, w)).collect();
        let assigns: Vec<Arg> = sc
            .assigns
            .iter()
            .map(|a| Arg {
                span: a.span,
                text: String::from(ast.text(a.span)),
                value: None,
                glob: false,
            })
            .collect();
        let idx = self.push(CommandInfo {
            span: cmd.span,
            source: cx.source,
            name: None,
            words: Vec::new(),
            assigns,
            redirects: Vec::new(),
            parent: cx.parent,
            via: cx.via,
            categories: Categories::empty(),
            reason: None,
        });
        let inner = cx.nested(idx, Via::CommandSubst);
        for a in &sc.assigns {
            if rules_dangerous(ast.text(a.name)) {
                self.mark_unknown(idx, "sets a variable that changes what runs");
            }
            if let Some(i) = &a.index {
                self.word(ast, i, inner, false);
            }
            match &a.value {
                AssignValue::Scalar(w) => self.word(ast, w, inner, false),
                AssignValue::Array(arr) => {
                    for e in &arr.elems {
                        if let Some(k) = &e.key {
                            self.word(ast, k, inner, false);
                        }
                        self.word(ast, &e.value, inner, false);
                    }
                }
            }
        }
        for w in &sc.words {
            self.word(ast, w, inner, false);
        }
        let (cats, infos) = self.redirects(ast, &cmd.redirects, inner, Some(idx));
        let stdin = stdin_of(ast, &cmd.redirects);
        {
            let c = &mut self.out.commands[idx];
            c.categories.extend(cats);
            c.redirects = infos;
        }
        if words.is_empty() {
            self.out.commands[idx].categories.insert(Category::Read);
            return;
        }
        self.apply(idx, words, cx, piped, stdin);
    }

    /// Classifies the command `idx` from its words and handles what it runs.
    fn apply(&mut self, idx: usize, words: Vec<Arg>, cx: Cx, piped: bool, stdin: Stdin) {
        let first = &words[0];
        let outcome = match (&first.value, first.glob) {
            (Some(v), false) => match rules::command_name(v) {
                Some(name) => {
                    self.out.commands[idx].name = Some(String::from(name));
                    let name = String::from(name);
                    let o = rules::classify(&name, &words);
                    if piped && truncates(&name, &words) {
                        self.out.truncated = true;
                    }
                    if name == "alias" {
                        for w in &words[1..] {
                            if let Some((n, _)) = w.value.as_deref().and_then(|v| v.split_once('='))
                            {
                                self.aliases.insert(String::from(n));
                            }
                        }
                    }
                    o
                }
                None => Outcome {
                    cats: Category::Unknown.into(),
                    reason: Some("runs a program given by its path"),
                    ..Outcome::default()
                },
            },
            _ => Outcome {
                cats: Category::Unknown.into(),
                reason: Some("the command name is dynamic"),
                ..Outcome::default()
            },
        };
        {
            let c = &mut self.out.commands[idx];
            c.categories.extend(outcome.cats);
            if outcome.reason.is_some() && c.reason.is_none() {
                c.reason = outcome.reason;
            }
        }
        for r in &outcome.reads {
            let e = effect(&words, r, cx.source, idx);
            self.out.reads.push(e);
        }
        for w in &outcome.writes {
            let e = effect(&words, w, cx.source, idx);
            if !e.path.as_deref().is_some_and(harmless_path) {
                self.out.writes.push(e);
            }
        }
        for n in &outcome.nested {
            match n {
                Nested::Words { start, end, via } => {
                    let sub: Vec<Arg> = words[*start..*end].to_vec();
                    let span = sub[0].span.to(sub[sub.len() - 1].span);
                    let child = self.push(CommandInfo {
                        span,
                        source: cx.source,
                        name: None,
                        words: Vec::new(),
                        assigns: Vec::new(),
                        redirects: Vec::new(),
                        parent: Some(idx),
                        via: *via,
                        categories: Categories::empty(),
                        reason: None,
                    });
                    self.apply(child, sub, cx.nested(child, *via), false, Stdin::Inherited);
                    // File names spliced into shell code (`xargs -I{} sh -c '… {} …'`,
                    // `find -exec sh -c '… {} …'`) can inject commands.
                    let c = &self.out.commands[child];
                    let shell = c
                        .name
                        .as_deref()
                        .is_some_and(|n| rules::SHELLS.contains(&n) || n == "eval");
                    // The `-c` string, or the `eval` arguments.
                    let code = c.words.iter().enumerate().skip(1).find_map(|(k, w)| {
                        let v = w.value.as_deref()?;
                        let opt = v.starts_with('-') && !v.starts_with("--") && v.contains('c');
                        if c.name.as_deref() == Some("eval") {
                            Some(v)
                        } else if opt {
                            c.words.get(k + 1).and_then(|n| n.value.as_deref())
                        } else {
                            None
                        }
                    });
                    let spliced = *via == Via::Xargs || code.is_some_and(|v| v.contains("{}"));
                    if shell && spliced && matches!(via, Via::Xargs | Via::FindExec) {
                        self.mark_unknown(child, "file names are spliced into shell code");
                    }
                }
                Nested::Source {
                    first,
                    last,
                    text,
                    via,
                } => {
                    let span = words[*first].span.to(words[*last].span);
                    self.source(text.clone(), span, cx.nested(idx, *via), idx);
                }
                Nested::Stdin => match &stdin {
                    Stdin::Literal(text, span) => {
                        self.source(text.clone(), *span, cx.nested(idx, Via::ShellString), idx);
                    }
                    Stdin::Dynamic => self.mark_unknown(
                        idx,
                        "a shell runs a script from a file or a dynamic here-document",
                    ),
                    Stdin::Inherited => {
                        self.mark_unknown(idx, "a shell runs a script from its input")
                    }
                },
            }
        }
        self.out.commands[idx].words = words;
    }

    /// Shell code in a string: parsed and walked as a script of its own.
    fn source(&mut self, text: String, span: Span, cx: Cx, owner: usize) {
        if cx.depth >= MAX_DEPTH {
            self.mark_unknown(owner, "shell strings nested too deeply");
            return;
        }
        match psh_syntax::parse(&text) {
            Ok(ast) => {
                self.out.sources.push(NestedSource {
                    span,
                    source: cx.source,
                    text,
                });
                let inner = Cx {
                    source: Some(self.out.sources.len() - 1),
                    depth: cx.depth + 1,
                    ..cx
                };
                if cx.via == Via::Backtick {
                    // A command substitution sees the shell's variables.
                    self.list(&ast, ast.root(), inner);
                } else {
                    // Another shell (`bash -c`) or a later time (`trap`, aliases): what is
                    // known here does not hold there.
                    let vars = core::mem::replace(&mut self.vars, Vars::scan(&ast));
                    let defined = core::mem::take(&mut self.defined);
                    self.list(&ast, ast.root(), inner);
                    self.vars = vars;
                    self.defined = defined;
                }
            }
            Err(_) => self.mark_unknown(owner, "a shell string does not parse"),
        }
    }

    /// Commands nested in a word: `$(…)`, backquotes, `<(…)`, and the words inside
    /// parameter expansions, arithmetic, braces and arrays.
    fn word(&mut self, ast: &Ast, w: &Word, cx: Cx, in_dq: bool) {
        self.parts(ast, &w.parts, cx, in_dq);
    }

    fn parts(&mut self, ast: &Ast, parts: &[WordPart], cx: Cx, in_dq: bool) {
        for p in parts {
            match &p.kind {
                Part::Double(inner) | Part::Locale(inner) => self.parts(ast, inner, cx, true),
                Part::Param(param) => self.param(ast, param, cx),
                Part::CmdSubst(l) | Part::FunSub { list: l, .. } => self.list(
                    ast,
                    *l,
                    Cx {
                        via: Via::CommandSubst,
                        ..cx
                    },
                ),
                Part::ProcSubst { list, .. } => self.list(
                    ast,
                    *list,
                    Cx {
                        via: Via::ProcessSubst,
                        ..cx
                    },
                ),
                Part::Backtick => {
                    let body = backtick_body(ast.text(p.span), in_dq);
                    let owner = cx.parent;
                    match owner {
                        Some(o) => self.source(
                            body,
                            p.span,
                            Cx {
                                via: Via::Backtick,
                                ..cx
                            },
                            o,
                        ),
                        None => {
                            // A backquote outside any simple command (a `for` list, a case
                            // word): walk it with no owner to blame on failure.
                            if let Ok(ast2) = psh_syntax::parse(&body) {
                                self.out.sources.push(NestedSource {
                                    span: p.span,
                                    source: cx.source,
                                    text: body,
                                });
                                let inner = Cx {
                                    source: Some(self.out.sources.len() - 1),
                                    via: Via::Backtick,
                                    depth: cx.depth + 1,
                                    ..cx
                                };
                                self.list(&ast2, ast2.root(), inner);
                            } else {
                                self.extra.insert(Category::Unknown);
                            }
                        }
                    }
                }
                Part::Arith(w) => self.word(ast, w, cx, false),
                Part::Brace(b) => {
                    if let Brace::Alts(ws) = &**b {
                        for w in ws {
                            self.word(ast, w, cx, in_dq);
                        }
                    }
                }
                Part::ExtGlob { alts, .. } => {
                    for w in alts {
                        self.word(ast, w, cx, in_dq);
                    }
                }
                Part::Array(arr) => {
                    for e in &arr.elems {
                        if let Some(k) = &e.key {
                            self.word(ast, k, cx, false);
                        }
                        self.word(ast, &e.value, cx, false);
                    }
                }
                Part::Lit | Part::Escaped | Part::Single | Part::AnsiC | Part::BadSubst => {}
            }
        }
    }

    fn param(&mut self, ast: &Ast, p: &Param, cx: Cx) {
        if let Some(Subscript::Expr(w)) = &p.index {
            self.word(ast, w, cx, false);
        }
        match &p.op {
            ParamOp::None | ParamOp::Transform(_) | ParamOp::Names { .. } => {}
            ParamOp::Default { kind, word, .. } => {
                if *kind == DefaultKind::Assign && rules_dangerous(ast.text(p.name_span)) {
                    match cx.parent {
                        Some(i) => {
                            self.mark_unknown(i, "assigns a variable that changes what runs")
                        }
                        None => self.extra.insert(Category::Unknown),
                    }
                }
                self.word(ast, word, cx, true);
            }
            ParamOp::RemovePrefix { pattern, .. } | ParamOp::RemoveSuffix { pattern, .. } => {
                self.word(ast, pattern, cx, true)
            }
            ParamOp::Replace {
                pattern,
                replacement,
                ..
            } => {
                self.word(ast, pattern, cx, true);
                if let Some(r) = replacement {
                    self.word(ast, r, cx, true);
                }
            }
            ParamOp::Case { pattern, .. } => {
                if let Some(pat) = pattern {
                    self.word(ast, pat, cx, true);
                }
            }
            ParamOp::Substring { offset, length } => {
                self.word(ast, offset, cx, false);
                if let Some(l) = length {
                    self.word(ast, l, cx, false);
                }
            }
        }
    }

    /// The effects of redirections; nested commands in their targets and here-document
    /// bodies are walked too.
    fn redirects(
        &mut self,
        ast: &Ast,
        redirects: &[Redirect],
        cx: Cx,
        owner: Option<usize>,
    ) -> (Categories, Vec<RedirInfo>) {
        let mut cats = Categories::empty();
        let mut infos = Vec::new();
        for r in redirects {
            let fd = match r.fd {
                Some(RedirFd::Number(n)) => Some(n),
                _ => None,
            };
            let target = match &r.target {
                RedirTarget::Word(w) => {
                    self.word(ast, w, cx, false);
                    Some(self.arg(ast, w))
                }
                RedirTarget::HereDoc(h) => {
                    let doc = ast.heredoc(*h);
                    if !doc.quoted {
                        self.word(ast, &doc.body, cx, true);
                    }
                    None
                }
            };
            infos.push(RedirInfo {
                span: r.span,
                fd,
                op: r.op,
                target: target.clone(),
            });
            let Some(t) = target else { continue };
            let effect = PathEffect {
                span: t.span,
                source: cx.source,
                path: t.value.clone(),
                command: owner,
            };
            let (read, write) = match r.op {
                RedirOp::Read => (true, false),
                RedirOp::ReadWrite => (true, true),
                RedirOp::Write
                | RedirOp::Append
                | RedirOp::Clobber
                | RedirOp::WriteBoth
                | RedirOp::AppendBoth => (false, true),
                // `>&2`, `<&0`, `>&-` duplicate fds; `>& file` writes a file.
                RedirOp::DupWrite | RedirOp::DupRead => {
                    let dup = t
                        .value
                        .as_deref()
                        .is_some_and(|v| v == "-" || v.bytes().all(|b| b.is_ascii_digit()));
                    (false, !dup && r.op == RedirOp::DupWrite)
                }
                RedirOp::HereString | RedirOp::HereDoc | RedirOp::HereDocStrip => (false, false),
            };
            if read && !effect.path.as_deref().is_some_and(harmless_path) {
                self.out.reads.push(effect.clone());
            }
            if write && !effect.path.as_deref().is_some_and(harmless_path) {
                cats.insert(Category::Mutate);
                self.out.writes.push(effect);
            }
        }
        (cats, infos)
    }
}

fn rules_dangerous(name: &str) -> bool {
    crate::dangerous_var(name)
}

fn arg(ast: &Ast, w: &Word) -> Arg {
    // `*`, `?` or a bracket expression in unquoted text (a lone `[` is not a pattern).
    let glob = w.parts.iter().any(|p| {
        let t = ast.text(p.span);
        matches!(p.kind, Part::Lit)
            && (t.contains(['*', '?']) || t.find('[').is_some_and(|i| t[i + 1..].contains(']')))
    });
    Arg {
        span: w.span,
        text: String::from(ast.text(w.span)),
        value: literal_text(ast, w),
        glob,
    }
}

fn effect(words: &[Arg], r: &PathRef, source: Option<usize>, command: usize) -> PathEffect {
    PathEffect {
        span: words[r.arg].span,
        source,
        path: r.value.clone(),
        command: Some(command),
    }
}

/// The script a shell would read on stdin.
fn stdin_of(ast: &Ast, redirects: &[Redirect]) -> Stdin {
    let mut stdin = Stdin::Inherited;
    for r in redirects {
        let to_stdin = matches!(r.fd, None | Some(RedirFd::Number(0)));
        if !to_stdin {
            continue;
        }
        stdin = match (&r.op, &r.target) {
            (RedirOp::HereDoc | RedirOp::HereDocStrip, RedirTarget::HereDoc(h)) => {
                let doc = ast.heredoc(*h);
                match literal_text(ast, &doc.body) {
                    Some(t) => Stdin::Literal(t, doc.body.span),
                    None => Stdin::Dynamic,
                }
            }
            (RedirOp::HereString, RedirTarget::Word(w)) => match literal_text(ast, w) {
                Some(mut t) => {
                    t.push('\n');
                    Stdin::Literal(t, w.span)
                }
                None => Stdin::Dynamic,
            },
            (RedirOp::Read | RedirOp::ReadWrite | RedirOp::DupRead, _) => Stdin::Dynamic,
            _ => continue,
        };
    }
    stdin
}

/// A pipeline stage that cuts its input short.
fn truncates(name: &str, words: &[Arg]) -> bool {
    let vals = || words[1..].iter().filter_map(|a| a.value.as_deref());
    match name {
        "head" => true,
        "tail" => !vals().any(|v| {
            v == "-f"
                || v == "-F"
                || v == "--follow"
                || (v.starts_with('-') && !v.starts_with("--") && v.contains(['f', 'F']))
        }),
        "sed" => vals().any(|v| {
            v == "-n"
                || v == "--quiet"
                || v == "--silent"
                || (v.starts_with('-') && !v.starts_with("--") && v.contains('n'))
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::{Category, Via, classify};
    use alloc::vec::Vec;

    #[test]
    fn nested_strings_have_their_own_source() {
        let c = classify("bash -c 'rm x'");
        assert_eq!(c.commands.len(), 2);
        assert_eq!(c.sources.len(), 1);
        assert_eq!(c.sources[0].text, "rm x");
        let rm = &c.commands[1];
        assert_eq!(
            (rm.source, rm.parent, rm.via),
            (Some(0), Some(0), Via::ShellString)
        );
        assert_eq!(&c.sources[0].text[rm.span.range()], "rm x");
        assert_eq!(c.writes[0].path.as_deref(), Some("x"));
        assert_eq!(c.writes[0].source, Some(0));
    }

    #[test]
    fn wrappers_and_substitutions_have_parents() {
        let c = classify("timeout 5 cargo test && echo $(ls)");
        let names: Vec<_> = c
            .commands
            .iter()
            .map(|x| (x.name.as_deref(), x.parent, x.via))
            .collect();
        assert_eq!(
            names,
            [
                (Some("timeout"), None, Via::Script),
                (Some("cargo"), Some(0), Via::Wrapper),
                (Some("echo"), None, Via::Script),
                (Some("ls"), Some(2), Via::CommandSubst),
            ]
        );
        assert_eq!(c.commands[1].primary(), Category::Build);
    }

    #[test]
    fn words_and_redirections_keep_spans() {
        let src = "grep -n \"$p\" f.txt 2>/dev/null > out";
        let c = classify(src);
        let g = &c.commands[0];
        let words: Vec<_> = g
            .words
            .iter()
            .map(|w| (&src[w.span.range()], w.value.as_deref()))
            .collect();
        assert_eq!(
            words,
            [
                ("grep", Some("grep")),
                ("-n", Some("-n")),
                ("\"$p\"", None),
                ("f.txt", Some("f.txt"))
            ]
        );
        assert_eq!(g.redirects.len(), 2);
        assert_eq!(g.redirects[0].fd, Some(2));
        assert_eq!(c.writes.len(), 1);
        assert_eq!(&src[c.writes[0].span.range()], "out");
        assert!(c.reads.iter().any(|r| r.path.as_deref() == Some("f.txt")));
    }
}
