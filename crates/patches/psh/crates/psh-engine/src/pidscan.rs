//! Whether a script reads a process id psh cannot give (D1413): `$$` (any parameter
//! expansion of `$`) or `BASHPID`. A `bash`/`sh` psh would run itself has no process id,
//! so a script that uses one runs under the real program instead, when there is one.
//!
//! The scan is over the parsed script: every command of every chunk (command and
//! process substitutions are commands of the same tree), their words, redirections and
//! here-documents; backquoted text and the literal script of a nested `bash -c '…'` are
//! parsed and scanned too. It errs on the side of finding a use: a literal `BASHPID`
//! anywhere counts (arithmetic names variables without `$`). What only appears at run
//! time (`eval`, `source`, `${!name}`) is caught when it expands instead.

use psh_syntax::{
    AssignValue, Ast, Brace, CmdId, CommandKind, CondExpr, CondKind, Options, Param, ParamName,
    ParamOp, Parser, Part, RedirTarget, Subscript, Word, WordPart,
};

/// How deep nested `bash -c` literals and backquotes are followed.
const MAX_DEPTH: u32 = 8;

/// Whether `source` reads `$$` or `BASHPID`. A syntax error ends the scan: what parses
/// up to it decides (psh reports the error when it runs the script).
pub(crate) fn uses_pid(source: &[u8]) -> bool {
    scan_source(source, 0)
}

fn scan_source(source: &[u8], depth: u32) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    // extglob on: more scripts parse, and a pattern never hides a `$$`.
    let mut parser = Parser::from_bytes(source, Options { extglob: true });
    while let Some(Ok(ast)) = parser.next_chunk() {
        let mut s = Scan {
            ast: &ast,
            depth,
            found: false,
        };
        for id in ast.cmd_ids() {
            s.command(id);
            if s.found {
                return true;
            }
        }
    }
    false
}

struct Scan<'a> {
    ast: &'a Ast,
    depth: u32,
    found: bool,
}

impl Scan<'_> {
    fn command(&mut self, id: CmdId) {
        let ast = self.ast;
        let cmd = ast.cmd(id);
        for r in &cmd.redirects {
            match &r.target {
                RedirTarget::Word(w) => self.word(w),
                RedirTarget::HereDoc(h) => {
                    let doc = ast.heredoc(*h);
                    if !doc.quoted {
                        self.word(&doc.body);
                    }
                }
            }
        }
        match &cmd.kind {
            CommandKind::Simple(s) => {
                for a in &s.assigns {
                    if let Some(i) = &a.index {
                        self.word(i);
                    }
                    match &a.value {
                        AssignValue::Scalar(w) => self.word(w),
                        AssignValue::Array(arr) => {
                            for e in &arr.elems {
                                if let Some(k) = &e.key {
                                    self.word(k);
                                }
                                self.word(&e.value);
                            }
                        }
                    }
                }
                for w in &s.words {
                    self.word(w);
                }
                self.nested_script(&s.words);
            }
            CommandKind::For(f) | CommandKind::Select(f) => {
                for w in f.words.iter().flatten() {
                    self.word(w);
                }
            }
            CommandKind::ArithFor(f) => {
                for w in [&f.init, &f.cond, &f.step] {
                    self.word(w);
                }
            }
            CommandKind::Case(c) => {
                self.word(&c.word);
                for item in &c.items {
                    for p in &item.patterns {
                        self.word(p);
                    }
                }
            }
            CommandKind::Arith(w) => self.word(w),
            CommandKind::Cond(c) => self.cond(c),
            // Their bodies are commands of the tree.
            CommandKind::Brace(_)
            | CommandKind::Subshell(_)
            | CommandKind::If(_)
            | CommandKind::While(_)
            | CommandKind::Function(_)
            | CommandKind::Coproc(_) => {}
        }
    }

    fn cond(&mut self, c: &CondExpr) {
        match &c.kind {
            CondKind::Unary { arg, .. } => self.word(arg),
            CondKind::Binary { lhs, rhs, .. } => {
                self.word(lhs);
                self.word(rhs);
            }
            CondKind::Word(w) => self.word(w),
            CondKind::Not(e) | CondKind::Group(e) => self.cond(e),
            CondKind::And(a, b) | CondKind::Or(a, b) => {
                self.cond(a);
                self.cond(b);
            }
        }
    }

    fn word(&mut self, w: &Word) {
        self.parts(&w.parts);
    }

    fn parts(&mut self, parts: &[WordPart]) {
        for p in parts {
            if self.found {
                return;
            }
            match &p.kind {
                Part::Lit => {
                    if has_name(self.ast.bytes(p.span), b"BASHPID") {
                        self.found = true;
                    }
                }
                Part::Double(inner) | Part::Locale(inner) => self.parts(inner),
                Part::Param(param) => self.param(param),
                Part::Arith(w) => self.word(w),
                Part::Backtick => {
                    let body = psh_syntax::backtick_body(self.ast.text(p.span), false);
                    if scan_source(body.as_bytes(), self.depth + 1) {
                        self.found = true;
                    }
                }
                Part::Brace(b) => match &**b {
                    Brace::Alts(words) => words.iter().for_each(|w| self.word(w)),
                    Brace::Seq { .. } => {}
                },
                Part::ExtGlob { alts, .. } => alts.iter().for_each(|w| self.word(w)),
                Part::Array(arr) => {
                    for e in &arr.elems {
                        if let Some(k) = &e.key {
                            self.word(k);
                        }
                        self.word(&e.value);
                    }
                }
                // Commands of the tree.
                Part::CmdSubst(_) | Part::FunSub { .. } | Part::ProcSubst { .. } => {}
                Part::Escaped | Part::Single | Part::AnsiC | Part::BadSubst => {}
            }
        }
    }

    fn param(&mut self, p: &Param) {
        let name = self.ast.bytes(p.name_span);
        if matches!(p.name, ParamName::Special(b'$')) || name == b"BASHPID" {
            self.found = true;
            return;
        }
        if let Some(Subscript::Expr(w)) = &p.index {
            self.word(w);
        }
        match &p.op {
            ParamOp::None | ParamOp::Transform(_) | ParamOp::Names { .. } => {}
            ParamOp::Default { word, .. } => self.word(word),
            ParamOp::RemovePrefix { pattern, .. } | ParamOp::RemoveSuffix { pattern, .. } => {
                self.word(pattern)
            }
            ParamOp::Replace {
                pattern,
                replacement,
                ..
            } => {
                self.word(pattern);
                if let Some(r) = replacement {
                    self.word(r);
                }
            }
            ParamOp::Case { pattern, .. } => {
                if let Some(w) = pattern {
                    self.word(w);
                }
            }
            ParamOp::Substring { offset, length } => {
                self.word(offset);
                if let Some(l) = length {
                    self.word(l);
                }
            }
        }
    }

    /// `bash -c 'SCRIPT'` (also `sh`, a path to either, combined letters like `-ec`):
    /// the script when it is literal.
    fn nested_script(&mut self, words: &[Word]) {
        if self.found {
            return;
        }
        let lit = |w: &Word| psh_syntax::literal_text(self.ast, w);
        let Some(first) = words.first().and_then(lit) else {
            return;
        };
        let base = first.rsplit('/').next().unwrap_or(&first);
        if base != "bash" && base != "sh" {
            return;
        }
        let mut rest = words[1..].iter();
        while let Some(w) = rest.next() {
            let Some(a) = lit(w) else { return };
            if a == "--" || !a.starts_with(['-', '+']) {
                return;
            }
            if a.starts_with('-') && !a.starts_with("--") && a.contains('c') {
                if let Some(script) = rest.next().and_then(lit)
                    && scan_source(script.as_bytes(), self.depth + 1)
                {
                    self.found = true;
                }
                return;
            }
        }
    }
}

/// `name` in `text` as a whole identifier.
fn has_name(text: &[u8], name: &[u8]) -> bool {
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    text.windows(name.len()).enumerate().any(|(i, w)| {
        w == name
            && (i == 0 || !is_ident(text[i - 1]))
            && text.get(i + name.len()).is_none_or(|&b| !is_ident(b))
    })
}

#[cfg(test)]
mod tests {
    use super::uses_pid;

    #[test]
    fn finds_pid_uses() {
        for src in [
            "echo $$",
            "echo \"pid=$$\"",
            "echo ${$}",
            "echo ${#$}",
            "x=${y:-$$}",
            "echo $BASHPID",
            "echo $((BASHPID + 1))",
            "(( BASHPID > 0 ))",
            "x=$(echo $$)",
            "x=`echo $$`",
            "cat <<EOF\n$$\nEOF\n",
            "f() { kill $$; }",
            "bash -c 'echo $$'",
            "sh -ec \"echo \\$BASHPID\"",
            "echo ok\necho $$ > pidfile",
            "[[ $$ -gt 0 ]]",
            "case $$ in *) ;; esac",
            "a=( $$ )",
            "echo > /tmp/f.$$",
            "echo $$x",
        ] {
            assert!(uses_pid(src.as_bytes()), "{src:?}");
        }
    }

    #[test]
    fn ignores_other_text() {
        for src in [
            "echo '$$'",
            "echo \\$\\$",
            "cat <<'EOF'\n$$\nEOF\n",
            "echo $PPID $! $BASHPIDX",
            "# $$ in a comment",
            "bash -c \"$cmd\"",
            "if",
        ] {
            assert!(!uses_pid(src.as_bytes()), "{src:?}");
        }
    }
}
