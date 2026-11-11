//! Unit tests of the parser: structure (through `dump`), spans, errors, here-docs,
//! chunked parsing and the printer round trip.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::*;

fn d(src: &str) -> String {
    let ast = parse(src).unwrap_or_else(|e| panic!("{src:?}: {e}"));
    check_spans(&ast);
    check_round_trip(&ast);
    dump(&ast)
}

fn dx(src: &str) -> String {
    let ast = parse_with(src, Options { extglob: true }).unwrap();
    check_spans(&ast);
    dump(&ast)
}

fn err(src: &str) -> String {
    match parse(src) {
        Ok(ast) => panic!("{src:?} parsed: {}", dump(&ast)),
        Err(e) => e.message,
    }
}

fn check_round_trip(ast: &Ast) {
    let printed = print(ast);
    let again = parse(&printed).unwrap_or_else(|e| panic!("reparse of {printed:?}: {e}"));
    assert_eq!(dump(&again), dump(ast), "printed:\n{printed}");
}

/// Collects `kind: text` for every node, checking that spans nest: a child's span lies
/// inside its parent's.
struct Walk<'a> {
    ast: &'a Ast,
    lines: Vec<String>,
}

impl Walk<'_> {
    fn node(&mut self, kind: &str, span: Span, parent: Span) {
        let src = self.ast.source_bytes();
        assert!(
            span.start <= span.end && span.end as usize <= src.len(),
            "{kind} {span:?} in {} bytes",
            src.len()
        );
        // Spans never split a char of the text view.
        let info = self.ast.source_info();
        assert!(info.view().is_char_boundary(info.view_offset(span.start)));
        assert!(info.view().is_char_boundary(info.view_offset(span.end)));
        assert!(
            parent.contains(span)
                || self
                    .ast
                    .original_span(parent)
                    .contains(self.ast.original_span(span)),
            "{kind} {span:?} {:?} outside {parent:?} {:?}",
            self.ast.text(span),
            self.ast.text(parent)
        );
        self.lines.push(format!(
            "{kind}: {}",
            self.ast.text(span).replace('\n', "\\n")
        ));
    }

    fn list(&mut self, id: ListId, parent: Span) {
        let l = self.ast.list(id);
        self.node("list", l.span, parent);
        for ao in &l.items {
            self.node("and-or", ao.span, l.span);
            self.pipeline(&ao.first, ao.span);
            for (_, p) in &ao.rest {
                self.pipeline(p, ao.span);
            }
        }
    }

    fn pipeline(&mut self, p: &Pipeline, parent: Span) {
        self.node("pipeline", p.span, parent);
        if let Some(t) = p.time {
            self.node("time", t.span, p.span);
        }
        for st in &p.stages {
            self.cmd(st.cmd, p.span);
        }
    }

    fn cmd(&mut self, id: CmdId, parent: Span) {
        let ast = self.ast;
        let c = ast.cmd(id);
        let kind = match &c.kind {
            CommandKind::Simple(_) => "simple",
            CommandKind::Brace(_) => "brace",
            CommandKind::Subshell(_) => "subshell",
            CommandKind::If(_) => "if",
            CommandKind::While(w) if w.until => "until",
            CommandKind::While(_) => "while",
            CommandKind::For(_) => "for",
            CommandKind::Select(_) => "select",
            CommandKind::ArithFor(_) => "arith-for",
            CommandKind::Case(_) => "case",
            CommandKind::Arith(_) => "arith",
            CommandKind::Cond(_) => "cond",
            CommandKind::Function(_) => "function",
            CommandKind::Coproc(_) => "coproc",
        };
        self.node(kind, c.span, parent);
        let sp = c.span;
        match &c.kind {
            CommandKind::Simple(s) => {
                for a in &s.assigns {
                    self.assign(a, sp);
                }
                for w in &s.words {
                    self.word("word", w, sp);
                }
            }
            CommandKind::Brace(l) | CommandKind::Subshell(l) => self.list(*l, sp),
            CommandKind::If(i) => {
                for b in &i.branches {
                    self.node("if-branch", b.span, sp);
                    self.list(b.cond, b.span);
                    self.list(b.body, b.span);
                }
                if let Some(e) = i.else_body {
                    self.list(e, sp);
                }
            }
            CommandKind::While(w) => {
                self.list(w.cond, sp);
                self.list(w.body, sp);
            }
            CommandKind::For(f) | CommandKind::Select(f) => {
                self.word("var", &f.var, sp);
                for w in f.words.iter().flatten() {
                    self.word("word", w, sp);
                }
                self.list(f.body, sp);
            }
            CommandKind::ArithFor(f) => {
                self.word("init", &f.init, sp);
                self.word("cond", &f.cond, sp);
                self.word("step", &f.step, sp);
                self.list(f.body, sp);
            }
            CommandKind::Case(cs) => {
                self.word("subject", &cs.word, sp);
                for item in &cs.items {
                    self.node("case-item", item.span, sp);
                    for p in &item.patterns {
                        self.word("pattern", p, item.span);
                    }
                    if let Some(b) = item.body {
                        self.list(b, item.span);
                    }
                }
            }
            CommandKind::Arith(w) => self.word("arith-text", w, sp),
            CommandKind::Cond(e) => self.cond(e, sp),
            CommandKind::Function(f) => {
                self.word("name", &f.name, sp);
                self.cmd(f.body, sp);
            }
            CommandKind::Coproc(cp) => {
                if let Some(n) = &cp.name {
                    self.word("name", n, sp);
                }
                self.cmd(cp.body, sp);
            }
        }
        for r in &c.redirects {
            self.node("redirect", r.span, sp);
            if let Some(RedirFd::Var(v)) = r.fd {
                self.node("fd-var", v, r.span);
            }
            match &r.target {
                RedirTarget::Word(w) => self.word("target", w, r.span),
                RedirTarget::HereDoc(id) => {
                    let h = ast.heredoc(*id);
                    self.word("delimiter", &h.delimiter, r.span);
                    let all = Span::new(0, ast.source_bytes().len() as u32);
                    self.word("heredoc-body", &h.body, all);
                }
            }
        }
    }

    fn assign(&mut self, a: &Assign, parent: Span) {
        self.node("assign", a.span, parent);
        self.node("assign-name", a.name, a.span);
        if let Some(i) = &a.index {
            self.word("index", i, a.span);
        }
        match &a.value {
            AssignValue::Scalar(w) => self.word("value", w, a.span),
            AssignValue::Array(arr) => self.array(arr, a.span),
        }
    }

    fn array(&mut self, arr: &ArrayLit, parent: Span) {
        self.node("array", arr.span, parent);
        for e in &arr.elems {
            self.node("elem", e.span, arr.span);
            if let Some(k) = &e.key {
                self.word("key", k, e.span);
            }
            self.word("value", &e.value, e.span);
        }
    }

    fn cond(&mut self, e: &CondExpr, parent: Span) {
        self.node("cond-expr", e.span, parent);
        match &e.kind {
            CondKind::Unary { op_span, arg, .. } => {
                self.node("cond-op", *op_span, e.span);
                self.word("operand", arg, e.span);
            }
            CondKind::Binary {
                op_span, lhs, rhs, ..
            } => {
                self.word("operand", lhs, e.span);
                self.node("cond-op", *op_span, e.span);
                self.word("operand", rhs, e.span);
            }
            CondKind::Word(w) => self.word("operand", w, e.span),
            CondKind::Not(i) | CondKind::Group(i) => self.cond(i, e.span),
            CondKind::And(a, b) | CondKind::Or(a, b) => {
                self.cond(a, e.span);
                self.cond(b, e.span);
            }
        }
    }

    fn word(&mut self, kind: &str, w: &Word, parent: Span) {
        self.node(kind, w.span, parent);
        for p in &w.parts {
            self.part(p, w.span);
        }
    }

    fn part(&mut self, p: &WordPart, parent: Span) {
        let kind = match &p.kind {
            Part::Lit => "lit",
            Part::Escaped => "escaped",
            Part::Single => "single",
            Part::AnsiC => "ansi-c",
            Part::Double(_) => "double",
            Part::Locale(_) => "locale",
            Part::Param(_) => "param",
            Part::BadSubst => "bad-subst",
            Part::CmdSubst(_) => "cmd-subst",
            Part::FunSub { .. } => "funsub",
            Part::Backtick => "backtick",
            Part::Arith(_) => "arith-subst",
            Part::ProcSubst { .. } => "proc-subst",
            Part::Brace(_) => "brace-exp",
            Part::ExtGlob { .. } => "extglob",
            Part::Array(_) => "array-part",
        };
        self.node(kind, p.span, parent);
        let sp = p.span;
        match &p.kind {
            Part::Double(inner) | Part::Locale(inner) => {
                for i in inner {
                    self.part(i, sp);
                }
            }
            Part::Param(pe) => {
                self.node("param-name", pe.name_span, sp);
                if let Some(Subscript::Expr(w)) = &pe.index {
                    self.word("subscript", w, sp);
                }
                match &pe.op {
                    ParamOp::Default { word, .. } => self.word("operand", word, sp),
                    ParamOp::RemovePrefix { pattern, .. }
                    | ParamOp::RemoveSuffix { pattern, .. } => self.word("pattern", pattern, sp),
                    ParamOp::Replace {
                        pattern,
                        replacement,
                        ..
                    } => {
                        self.word("pattern", pattern, sp);
                        if let Some(r) = replacement {
                            self.word("replacement", r, sp);
                        }
                    }
                    ParamOp::Case {
                        pattern: Some(p), ..
                    } => self.word("pattern", p, sp),
                    ParamOp::Substring { offset, length } => {
                        self.word("offset", offset, sp);
                        if let Some(l) = length {
                            self.word("length", l, sp);
                        }
                    }
                    _ => {}
                }
            }
            Part::CmdSubst(l) | Part::FunSub { list: l, .. } => self.list(*l, sp),
            Part::ProcSubst { list, .. } => self.list(*list, sp),
            Part::Arith(w) => self.word("arith-text", w, sp),
            Part::Brace(b) => match &**b {
                Brace::Alts(alts) => {
                    for a in alts {
                        self.word("alt", a, sp);
                    }
                }
                Brace::Seq { start, end, step } => {
                    self.node("seq-start", *start, sp);
                    self.node("seq-end", *end, sp);
                    if let Some(s) = step {
                        self.node("seq-step", *s, sp);
                    }
                }
            },
            Part::ExtGlob { alts, .. } => {
                for a in alts {
                    self.word("alt", a, sp);
                }
            }
            Part::Array(arr) => self.array(arr, sp),
            _ => {}
        }
    }
}

fn walk(ast: &Ast) -> Vec<String> {
    let mut w = Walk {
        ast,
        lines: Vec::new(),
    };
    let all = Span::new(0, ast.source_bytes().len() as u32);
    w.list(ast.root(), all);
    w.lines
}

fn check_spans(ast: &Ast) {
    walk(ast);
}

#[test]
fn lists_and_pipelines() {
    assert_eq!(
        d("a | b |& c && ! d || time -p e; f &\ng"),
        "(list (andor (pipe (cmd <\"a\">) | (cmd <\"b\">) |& (cmd <\"c\">)) && (pipe ! (cmd <\"d\">)) || (pipe time-p (cmd <\"e\">))) (andor (cmd <\"f\">) &) (cmd <\"g\">))"
    );
    assert_eq!(d("time"), "(list (pipe time))");
    assert_eq!(d("! ! true"), "(list (pipe ! ! (cmd <\"true\">)))");
    assert_eq!(
        d("a &&\n\n b"),
        "(list (andor (cmd <\"a\">) && (cmd <\"b\">)))"
    );
    assert_eq!(d("a |\n b"), "(list (pipe (cmd <\"a\">) | (cmd <\"b\">)))");
    assert_eq!(d("# only a comment\n"), "(list)");
    assert_eq!(d(""), "(list)");
}

#[test]
fn simple_commands() {
    assert_eq!(
        d("A=1 B+=x C[i+1]=y cmd arg >out 2>&1"),
        "(list (cmd (assign A= <\"1\">) (assign B+= <\"x\">) (assign C[<\"i+1\">]= <\"y\">) <\"cmd\"> <\"arg\"> ><\"out\"> 2>&<\"1\">))"
    );
    // Assignments only before the command name.
    assert_eq!(d("cmd A=1"), "(list (cmd <\"cmd\"> <\"A=1\">))");
    assert_eq!(
        d("a=(1 [k]=v [j]+=w \"[x]=y\")"),
        "(list (cmd (assign a= (array <\"1\"> [<\"k\">]=<\"v\"> [<\"j\">]+=<\"w\"> <dq(\"[x]=y\")>))))"
    );
    assert_eq!(
        d("a[1 + 2]=x"),
        "(list (cmd (assign a[<\"1 + 2\">]= <\"x\">)))"
    );
    assert_eq!(d("a="), "(list (cmd (assign a= <>)))");
    assert_eq!(
        d("declare -A m=([a]=1) x=$y"),
        "(list (cmd <\"declare\"> <\"-A\"> <\"m=\" (array [<\"a\">]=<\"1\">)> <\"x=\" $y>))"
    );
    assert_eq!(
        d(">out <in 2>>log 3<>rw 4>|f &>both &>>both2 <&0 >&- {fd}>x 5<<<str"),
        "(list (cmd ><\"out\"> <<\"in\"> 2>><\"log\"> 3<><\"rw\"> 4>|<\"f\"> &><\"both\"> &>><\"both2\"> <&<\"0\"> >&<\"-\"> {fd}><\"x\"> 5<<<<\"str\">))"
    );
    // `2 >x` is an argument and a redirection.
    assert_eq!(d("echo 2 >x"), "(list (cmd <\"echo\"> <\"2\"> ><\"x\">))");
}

#[test]
fn compound_commands() {
    assert_eq!(
        d("if a; then b; elif c; then d; else e; fi"),
        "(list (if (list (cmd <\"a\">)) (list (cmd <\"b\">)) (list (cmd <\"c\">)) (list (cmd <\"d\">)) else (list (cmd <\"e\">))))"
    );
    assert_eq!(
        d("while a; do b; done; until c; do d; done"),
        "(list (while (list (cmd <\"a\">)) (list (cmd <\"b\">))) (until (list (cmd <\"c\">)) (list (cmd <\"d\">))))"
    );
    assert_eq!(
        d("for i in a b; do c; done; for j; do :; done; for k do :; done"),
        "(list (for <\"i\"> in <\"a\"> <\"b\">; (list (cmd <\"c\">))) (for <\"j\"> (list (cmd <\":\">))) (for <\"k\"> (list (cmd <\":\">))))"
    );
    assert_eq!(
        d("for i in; do :; done"),
        "(list (for <\"i\"> in; (list (cmd <\":\">))))"
    );
    assert_eq!(
        d("for x in a\ndo :; done; for y; { :; }"),
        "(list (for <\"x\"> in <\"a\">; (list (cmd <\":\">))) (for <\"y\"> (list (cmd <\":\">))))"
    );
    assert_eq!(
        d("for ((i=0; i<3; i++)); do :; done; for ((;;)) { :; }"),
        "(list (arith-for <\"i=0\"> <\" i<3\"> <\" i++\"> (list (cmd <\":\">))) (arith-for <> <> <> (list (cmd <\":\">))))"
    );
    assert_eq!(
        d("select x in a; do break; done"),
        "(list (select <\"x\"> in <\"a\">; (list (cmd <\"break\">))))"
    );
    assert_eq!(
        d("case $x in a|b) c;; (d) ;& e) f;;& *) esac"),
        "(list (case <$x> (<\"a\"> | <\"b\">) (list (cmd <\"c\">)) ;; (<\"d\">) ;& (<\"e\">) (list (cmd <\"f\">)) ;;& (<\"*\">) ;;))"
    );
    assert_eq!(d("case x in esac"), "(list (case <\"x\">))");
    assert_eq!(
        d("case x in\n  a)\n    b\n    ;;\nesac"),
        "(list (case <\"x\"> (<\"a\">) (list (cmd <\"b\">)) ;;))"
    );
    assert_eq!(
        d("{ a; b; } >f; (c) 2>/dev/null"),
        "(list (brace (list (cmd <\"a\">) (cmd <\"b\">)) ><\"f\">) (subshell (list (cmd <\"c\">)) 2><\"/dev/null\">))"
    );
    assert_eq!(d("{ a;}"), "(list (brace (list (cmd <\"a\">))))");
    assert_eq!(d("((x = 1 + 2))"), "(list (arith <\"x = 1 + 2\">))");
    // `((` that does not close with `))` is two subshells.
    assert_eq!(
        d("((a) )"),
        "(list (subshell (list (subshell (list (cmd <\"a\">))))))"
    );
    assert_eq!(
        d("f() { a; }; function g { b; } >out; function h() (c); i ( ) [[ x ]]"),
        "(list (fn <\"f\"> (brace (list (cmd <\"a\">)))) (function <\"g\"> (brace (list (cmd <\"b\">)) ><\"out\">)) (function <\"h\"> (subshell (list (cmd <\"c\">)))) (fn <\"i\"> (cond <\"x\">)))"
    );
    assert_eq!(
        d("f()\n{\n:\n}"),
        "(list (fn <\"f\"> (brace (list (cmd <\":\">)))))"
    );
    assert_eq!(
        d("coproc cat; coproc N { cat; }; coproc { cat; }"),
        "(list (coproc (cmd <\"cat\">)) (coproc <\"N\"> (brace (list (cmd <\"cat\">)))) (coproc (brace (list (cmd <\"cat\">)))))"
    );
    // Reserved words are only reserved in command position.
    assert_eq!(
        d("echo if then { } fi"),
        "(list (cmd <\"echo\"> <\"if\"> <\"then\"> <\"{\"> <\"}\"> <\"fi\">))"
    );
}

#[test]
fn conditionals() {
    assert_eq!(
        d("[[ -f x && ( $a == b* || ! c =~ ^(d|e)$ ) ]]"),
        "(list (cond ((-f <\"x\">) && (group ((<$a> == <\"b*\">) || (! (<\"c\"> =~ <\"^(d|e)$\">)))))))"
    );
    assert_eq!(
        d("[[ a < b && c > d && x -nt y ]]"),
        "(list (cond (((<\"a\"> < <\"b\">) && (<\"c\"> > <\"d\">)) && (<\"x\"> -nt <\"y\">))))"
    );
    assert_eq!(d("[[ x ]]"), "(list (cond <\"x\">))");
    assert_eq!(
        d("[[ $x =~ (a b) ]]"),
        "(list (cond (<$x> =~ <\"(a b)\">)))"
    );
    // extglob is always on for the right side of `==`.
    assert_eq!(
        d("[[ x == @(a|b) ]]"),
        "(list (cond (<\"x\"> == <@(<\"a\">|<\"b\">)>)))"
    );
    assert_eq!(
        d("[[\n a == b\n &&\n c == d\n]]"),
        "(list (cond ((<\"a\"> == <\"b\">) && (<\"c\"> == <\"d\">))))"
    );
}

#[test]
fn words() {
    assert_eq!(
        d(r#"echo a\ b 'c d' "e $f ${g}" $'h\n' $"i""#),
        r#"(list (cmd <"echo"> <"a" \  "b"> <'c d'> <dq("e " $f " " ${g})> <$'h\n'> <ldq("i")>))"#
    );
    assert_eq!(
        d(r#"echo "a\"b\$c\d""#),
        r#"(list (cmd <"echo"> <dq("a" \" "b" \$ "c\\d")>))"#
    );
    assert_eq!(
        d("echo $(a; b) `c` $((1 + $x)) $[2] <(d) >(e)"),
        "(list (cmd <\"echo\"> <$(list (cmd <\"a\">) (cmd <\"b\">))> <`c`> <arith<\"1 + \" $x>> <arith<\"2\">> <<(list (cmd <\"d\">))> <>(list (cmd <\"e\">))>))"
    );
    assert_eq!(
        d("echo $((a) )"),
        "(list (cmd <\"echo\"> <$(list (subshell (list (cmd <\"a\">))))>))"
    );
    assert_eq!(
        d("echo $0 $1 $10 ${10} $# $? $$ $! $- $@ $* $_ $ x$"),
        "(list (cmd <\"echo\"> <$0> <$1> <$1 \"0\"> <${10}> <$#> <$?> <$$> <$!> <$-> <$@> <$*> <$_> <\"$\"> <\"x$\">))"
    );
    assert_eq!(d("echo a\\\nb"), "(list (cmd <\"echo\"> <\"a\" \"b\">))");
    assert_eq!(d("echo a#b #c"), "(list (cmd <\"echo\"> <\"a#b\">))");
    assert_eq!(d("echo héllo"), "(list (cmd <\"echo\"> <\"héllo\">))");
    assert_eq!(d("echo \\é"), "(list (cmd <\"echo\"> <\\é>))");
    assert_eq!(
        d("x=${ echo a; } y=${| b; }"),
        "(list (cmd (assign x= <${ (list (cmd <\"echo\"> <\"a\">))}>) (assign y= <${|(list (cmd <\"b\">))}>)))"
    );
}

#[test]
fn parameters() {
    assert_eq!(
        d(
            "echo ${a:-b c} ${a-x} ${a:=y} ${a?z} ${a:+w} ${#a} ${#a[@]} ${!a} ${!a[*]} ${!p*} ${!p@}"
        ),
        "(list (cmd <\"echo\"> <${a :-<\"b c\">}> <${a -<\"x\">}> <${a :=<\"y\">}> <${a ?<\"z\">}> <${a :+<\"w\">}> <${#a}> <${#a[@]}> <${!a}> <${!a[*]}> <${!p*}> <${!p@}>))"
    );
    assert_eq!(
        d("echo ${a#x} ${a##x} ${a%y} ${a%%y} ${a/p/r} ${a//p} ${a/#p/r} ${a/%p/r}"),
        "(list (cmd <\"echo\"> <${a #<\"x\">}> <${a ##<\"x\">}> <${a %<\"y\">}> <${a %%<\"y\">}> <${a /<\"p\">/<\"r\">}> <${a //<\"p\">}> <${a /#<\"p\">/<\"r\">}> <${a /%<\"p\">/<\"r\">}>))"
    );
    // After `//`, a `/` starts the pattern (bash); after `/#` it ends an empty one.
    assert_eq!(
        d("echo ${x////c} ${x///} ${x///c/Z} ${x/#//X}"),
        "(list (cmd <\"echo\"> <${x //<\"/\">/<\"c\">}> <${x //<\"/\">}> <${x //<\"/c\">/<\"Z\">}> <${x /#<>/<\"/X\">}>))"
    );
    assert_eq!(
        d(
            "echo ${a:1} ${a: -1:2} ${a^} ${a^^} ${a,} ${a,,x} ${a~} ${a@Q} ${a[i+1]} ${#} ${##} ${!}"
        ),
        "(list (cmd <\"echo\"> <${a :<\"1\">}> <${a :<\" -1\">:<\"2\">}> <${a ^}> <${a ^^}> <${a ,}> <${a ,,<\"x\">}> <${a ~}> <${a @Q}> <${a[<\"i+1\">]}> <${#}> <${##}> <${!}>))"
    );
    // Errors in `${…}` are runtime errors in bash ("bad substitution").
    assert_eq!(
        d("echo ${a@Zz} ${1abc} ${#a:-x} ${}"),
        "(list (cmd <\"echo\"> <bad(${a@Zz})> <bad(${1abc})> <bad(${#a:-x})> <bad(${})>))"
    );
    assert_eq!(
        d("echo ${x:-{a}} ${x:-a}b}"),
        "(list (cmd <\"echo\"> <${x :-<\"{a\">} \"}\"> <${x :-<\"a\">} \"b}\">))"
    );
    assert_eq!(
        d(r#"echo "${x:-'a'}" "${x%d\'}" ${x:-"$y"}"#),
        r#"(list (cmd <"echo"> <dq(${x :-<'a'>})> <dq(${x %<"d" \'>})> <${x :-<dq($y)>}>))"#
    );
}

#[test]
fn brace_expansion() {
    assert_eq!(
        d("echo {a,b}c {1..3} {a..e..2} a{b,c{d,e}}f {x} {} {,x} \"{a,b}\" {a,\"b c\"}"),
        "(list (cmd <\"echo\"> <brace(<\"a\">,<\"b\">) \"c\"> <seq(1..3)> <seq(a..e..2)> <\"a\" brace(<\"b\">,<\"c\" brace(<\"d\">,<\"e\">)>) \"f\"> <\"{x}\"> <\"{}\"> <brace(<>,<\"x\">)> <dq(\"{a,b}\")> <brace(<\"a\">,<dq(\"b c\")>)>))"
    );
    // Not in assignments, here-strings or case words.
    assert_eq!(
        d("a={x,y} cat <<<{c,d}"),
        "(list (cmd (assign a= <\"{x,y}\">) <\"cat\"> <<<<\"{c,d}\">))"
    );
    assert_eq!(
        d("echo {1..a} {a..bb}"),
        "(list (cmd <\"echo\"> <\"{1..a}\"> <\"{a..bb}\">))"
    );
}

#[test]
fn extglob() {
    // Without extglob, `@(` is a syntax error, as in bash.
    assert_eq!(err("echo @(a|b)"), "syntax error near unexpected token `('");
    assert_eq!(
        dx("echo @(a|b) !(x) *(y|z*) +(@(p)) ?(q r)"),
        "(list (cmd <\"echo\"> <@(<\"a\">|<\"b\">)> <!(<\"x\">)> <*(<\"y\">|<\"z*\">)> <+(<@(<\"p\">)>)> <?(<\"q r\">)>))"
    );
    assert_eq!(
        dx("case x in @(a|b)) ;; esac"),
        "(list (case <\"x\"> (<@(<\"a\">|<\"b\">)>) ;;))"
    );
    // In an extglob, a `$(` ends at the paren that balances it, even one in what would
    // be a comment; its text is parsed only up to there (bash).
    assert_eq!(
        dx("echo @($(echo \"a)\"))z"),
        "(list (cmd <\"echo\"> <@(<$(list (cmd <\"echo\"> <dq(\"a)\")>))>) \"z\">))"
    );
    assert_eq!(
        dx("echo @($(echo x # ))"),
        "(list (cmd <\"echo\"> <@(<$(list (cmd <\"echo\"> <\"x\">))>)>))"
    );
    let e = parse_with("echo @($(echo x # )\n))y", Options { extglob: true }).unwrap_err();
    assert_eq!(e.message, "syntax error near unexpected token `)'");
}

#[test]
fn heredocs() {
    assert_eq!(
        d("cat <<EOF; cat <<-'E2'\nhello $x\nEOF\n\ttab\n\tE2\necho done"),
        "(list (cmd <\"cat\"> <<<\"EOF\">[\"hello \" $x \"\\n\"]) (cmd <\"cat\"> <<-<'E2'>[quoted \"tab\\n\"]) (cmd <\"echo\"> <\"done\">))"
    );
    assert_eq!(
        d("cat <<A <<B\na\nA\nb\nB"),
        "(list (cmd <\"cat\"> <<<\"A\">[\"a\\n\"] <<<\"B\">[\"b\\n\"]))"
    );
    // Body of a here-doc inside `$(…)`, and of one whose command line continues in
    // a `$(…)` spanning lines.
    assert_eq!(
        d("x=$(cat <<EOF\ninner\nEOF\n)"),
        "(list (cmd (assign x= <$(list (cmd <\"cat\"> <<<\"EOF\">[\"inner\\n\"]))>)))"
    );
    assert_eq!(
        d("cat <<A; echo \"$(echo x\necho y)\"\na\nA"),
        "(list (cmd <\"cat\"> <<<\"A\">[\"a\\n\"]) (cmd <\"echo\"> <dq($(list (cmd <\"echo\"> <\"x\">) (cmd <\"echo\"> <\"y\">)))>))"
    );
    // A missing delimiter line: the body runs to the end.
    let ast = parse("cat <<EOF\nbody").unwrap();
    assert!(!ast.heredoc(HereDocId(0)).terminated);
    assert_eq!(ast.text(ast.heredoc(HereDocId(0)).body.span), "body");
    let ast = parse("cat <<EOF").unwrap();
    assert!(!ast.heredoc(HereDocId(0)).terminated);
    // Errors in an unquoted body show when it is expanded, not as syntax errors.
    let ast = parse("cat <<EOF\na $(echo b\nEOF\necho after").unwrap();
    let h = ast.heredoc(HereDocId(0));
    assert_eq!(
        h.error.as_ref().unwrap().message,
        "unexpected EOF while looking for matching `)'"
    );
    assert_eq!(
        dump(&ast),
        "(list (cmd <\"cat\"> <<<\"EOF\">[\"a $(echo b\\n\"]) (cmd <\"echo\"> <\"after\">))"
    );
    // `<<-` strips tabs from the expansion text too.
    assert_eq!(
        d("cat <<-X\n\ta $y\n\t\tb\n\tX"),
        "(list (cmd <\"cat\"> <<-<\"X\">[\"a \" $y \"\\n\" \"b\\n\"]))"
    );
}

#[test]
fn errors() {
    let cases = [
        ("echo )", "syntax error near unexpected token `)'"),
        ("fi", "syntax error near unexpected token `fi'"),
        ("a ;; b", "syntax error near unexpected token `;;'"),
        ("echo a; ;", "syntax error near unexpected token `;'"),
        ("echo >", "syntax error near unexpected token `newline'"),
        ("echo > ;", "syntax error near unexpected token `;'"),
        (
            "if true; then fi",
            "syntax error near unexpected token `fi'",
        ),
        ("f() echo hi", "syntax error near unexpected token `echo'"),
        ("true | ! false", "syntax error near unexpected token `!'"),
        ("echo a=(1)", "syntax error near unexpected token `('"),
        (
            "if true",
            "syntax error: unexpected end of file from `if' command on line 1",
        ),
        (
            "\n{ echo }",
            "syntax error: unexpected end of file from `{' command on line 2",
        ),
        ("echo \"a", "unexpected EOF while looking for matching `\"'"),
        ("echo 'a", "unexpected EOF while looking for matching `''"),
        ("echo `a", "unexpected EOF while looking for matching ``'"),
        ("echo $(a", "unexpected EOF while looking for matching `)'"),
        ("echo ${a", "unexpected EOF while looking for matching `}'"),
        ("echo $((1", "unexpected EOF while looking for matching `)'"),
        ("echo $(if)", "syntax error near unexpected token `)'"),
        (
            "echo $(echo ;;)",
            "syntax error near unexpected token `;;' while looking for matching `)'",
        ),
        ("a &&", "syntax error: unexpected end of file"),
        (
            "for ((i=0;i<3)); do :; done",
            "syntax error: arithmetic expression required",
        ),
        (
            "[[ a -foo b ]]",
            "unexpected token `-foo', conditional binary operator expected\nsyntax error near `-foo'",
        ),
        (
            "[[ -x ]]",
            "unexpected argument `]]' to conditional unary operator\nsyntax error near `]]'",
        ),
        ("[[ ]]", "syntax error near `]]'"),
        (
            "[[ a =~ ; ]]",
            "syntax error in conditional expression: unexpected token `;'\nsyntax error near `;'",
        ),
        (
            "[[ a =~ <& ]]",
            "syntax error in conditional expression: unexpected token `<&'\nsyntax error near `&'",
        ),
        (
            "[[ a == >> ]]",
            "unexpected argument `>>' to conditional binary operator\nsyntax error near `>>'",
        ),
        (
            "[[ ;; ]]",
            "unexpected token `;;' in conditional command\nsyntax error near `;'",
        ),
    ];
    for (src, msg) in cases {
        assert_eq!(err(src), msg, "{src:?}");
    }
    // The span points at the offending token.
    let e = parse("echo a; fi; b").unwrap_err();
    assert_eq!(e.span, Span::new(8, 10));
    assert!(!e.incomplete);
    let e = parse("if true").unwrap_err();
    assert_eq!(e.span, Span::new(7, 7));
    assert!(e.incomplete);
}

#[test]
fn bash_accepts() {
    // Each of these parses in bash 5.3 (`bash -n`).
    for src in [
        "!",
        "! ;",
        "time ! true",
        "a=1 b=2",
        "> out",
        "echo `if`",
        "echo ${x@Z}",
        "[[ a =~ | ]]",
        "[[ a =~ || ]]",
        "[[ a =~ |x ]]",
        "echo $((1 + ))",
        "for 1 in a; do :; done",
        "true | time false",
        "coproc foo bar",
        "time -p",
        "echo a\\",
        "let x=( 1 )",
        "alias a=(1 2)",
        "echo $\\\n(echo a) $\\\n{x}",
        "true &\\\n& echo and",
        "case x in (esac) ;; esac",
        "[[ -f <(x) ]]",
        "echo $( (a) )",
        "x=$(cat <<EOF\nabc\nEOF)",
    ] {
        if let Err(e) = parse(src) {
            panic!("{src:?}: {e}");
        }
    }
}

#[test]
fn spans_cover_every_node_kind() {
    let src = "A=1 b[2]+=(x [k]=v) cmd \"q $p\" 'l' $'c' $\"t\" \\e ${v:-w} ${#v} ${1} \
               ${a[i]} $(s) ${ f; } `b` $((1+2)) <(r) {x,y} {1..2} ${v@Q} ${!} >o 2>&1 {fd}<i <<H\n\
               body $z\n\
               H\n\
               time ! a | b && c || d &\n\
               if i; then j; elif k; then l; else m; fi\n\
               while n; do o; done; until p; do q; done\n\
               for r in s; do t; done; select u in v; do w; done; for ((x=0;x<1;x++)); do y; done\n\
               case z in a|b) c;; esac; { d; }; (e); ((f)); [[ -f g && h == i || ! j ]]\n\
               fn() { k; }; coproc N { l; }\n\
               declare -a arr=(m)";
    let ast = parse_with(src, Options::default()).unwrap();
    let lines = walk(&ast);
    let has = |line: &str| {
        assert!(
            lines.iter().any(|l| l == line),
            "missing {line:?} in:\n{}",
            lines.join("\n")
        )
    };
    has("assign: A=1");
    has("assign-name: b");
    has("index: 2");
    has("array: (x [k]=v)");
    has("elem: [k]=v");
    has("key: k");
    has("double: \"q $p\"");
    has("param: $p");
    has("param-name: p");
    has("single: 'l'");
    has("ansi-c: $'c'");
    has("locale: $\"t\"");
    has("escaped: \\e");
    has("param: ${v:-w}");
    has("operand: w");
    has("param: ${#v}");
    has("param: ${1}");
    has("subscript: i");
    has("cmd-subst: $(s)");
    has("funsub: ${ f; }");
    has("backtick: `b`");
    has("arith-subst: $((1+2))");
    has("arith-text: 1+2");
    has("proc-subst: <(r)");
    has("brace-exp: {x,y}");
    has("alt: x");
    has("brace-exp: {1..2}");
    has("seq-start: 1");
    has("seq-end: 2");
    has("param: ${v@Q}");
    has("param: ${!}");
    has("redirect: >o");
    has("redirect: 2>&1");
    has("fd-var: fd");
    has("redirect: <<H");
    has("delimiter: H");
    has("heredoc-body: body $z\\n");
    has("pipeline: time ! a | b");
    has("time: time");
    has("and-or: time ! a | b && c || d");
    has("if: if i; then j; elif k; then l; else m; fi");
    has("if-branch: elif k; then l; ");
    has("while: while n; do o; done");
    has("until: until p; do q; done");
    has("for: for r in s; do t; done");
    has("var: r");
    has("select: select u in v; do w; done");
    has("arith-for: for ((x=0;x<1;x++)); do y; done");
    has("init: x=0");
    has("step: x++");
    has("case: case z in a|b) c;; esac");
    has("case-item: a|b) c;;");
    has("pattern: b");
    has("brace: { d; }");
    has("subshell: (e)");
    has("arith: ((f))");
    has("cond: [[ -f g && h == i || ! j ]]");
    has("cond-expr: -f g && h == i");
    has("cond-op: ==");
    has("cond-expr: ! j");
    has("function: fn() { k; }");
    has("name: fn");
    has("coproc: coproc N { l; }");
    has("array-part: (m)");
    has("simple: declare -a arr=(m)");
    has("list: d");
    // Every node kind of the AST appears at least once.
    for kind in [
        "list",
        "and-or",
        "pipeline",
        "time",
        "simple",
        "brace",
        "subshell",
        "if",
        "while",
        "until",
        "for",
        "select",
        "arith-for",
        "case",
        "arith",
        "cond",
        "function",
        "coproc",
        "assign",
        "array",
        "elem",
        "redirect",
        "delimiter",
        "heredoc-body",
        "lit",
        "escaped",
        "single",
        "ansi-c",
        "double",
        "locale",
        "param",
        "cmd-subst",
        "funsub",
        "backtick",
        "arith-subst",
        "proc-subst",
        "brace-exp",
        "array-part",
        "cond-expr",
        "cond-op",
    ] {
        assert!(
            lines.iter().any(|l| l.starts_with(&format!("{kind}: "))),
            "no {kind}"
        );
    }
    let ast = parse_with("echo @(a) ${x@}", Options { extglob: true }).unwrap();
    let lines = walk(&ast);
    assert!(lines.iter().any(|l| l == "extglob: @(a)"));
    assert!(lines.iter().any(|l| l == "bad-subst: ${x@}"));
}

#[test]
fn line_index() {
    let src = "a\nb ç\nc";
    let ast = parse(src).unwrap();
    let idx = LineIndex::new(ast.source());
    let lc = idx.line_col(src, 6);
    assert_eq!((lc.line, lc.column), (2, 4));
}

#[test]
fn chunks() {
    let mut p = Parser::new(
        "echo a\nshopt -s extglob\necho @(b)\nfi\necho never",
        Options::default(),
    );
    let a = p.next_chunk().unwrap().unwrap();
    assert_eq!(dump(&a), "(list (cmd <\"echo\"> <\"a\">))");
    let b = p.next_chunk().unwrap().unwrap();
    assert_eq!(a.text(a.list(a.root()).span), "echo a");
    assert_eq!(b.text(b.list(b.root()).span), "shopt -s extglob");
    p.set_options(Options { extglob: true });
    let c = p.next_chunk().unwrap().unwrap();
    assert_eq!(dump(&c), "(list (cmd <\"echo\"> <@(<\"b\">)>))");
    let e = p.next_chunk().unwrap().unwrap_err();
    assert_eq!(e.message, "syntax error near unexpected token `fi'");
    assert!(p.next_chunk().is_none());

    // A chunk is a whole line, here-doc bodies included.
    let mut p = Parser::new("cat <<E; echo x\nbody\nE\necho y\n\n", Options::default());
    let a = p.next_chunk().unwrap().unwrap();
    assert_eq!(a.list(a.root()).items.len(), 2);
    let b = p.next_chunk().unwrap().unwrap();
    assert_eq!(dump(&b), "(list (cmd <\"echo\"> <\"y\">))");
    assert!(p.next_chunk().is_none());
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let deep = "( ".repeat(5000);
    assert_eq!(err(&deep), "nesting too deep");
    let deep = "$(".repeat(5000);
    assert_eq!(err(&format!("echo {deep}")), "nesting too deep");
    let ok = format!("{}a{}", "(".repeat(50), ")".repeat(50));
    d(&ok);
}

#[test]
fn printer_layout() {
    let ast = parse("if a; then b; fi; for i in 1 2; do echo $i | cat <<E; done\nx\nE").unwrap();
    assert_eq!(
        print(&ast),
        "if\n  a\nthen\n  b\nfi\nfor i in 1 2;\ndo\n  echo $i | cat <<E\nx\nE\ndone\n"
    );
    let ast = parse("cat < <(ls) << -X\nb\n-X").unwrap();
    assert_eq!(print(&ast), "cat < <(ls) << -X\nb\n-X\n");
    assert_eq!(
        d("tr a b <<<\"[$(cat <<EOF)]\"\nbody\nEOF\necho after"),
        "(list (cmd <\"tr\"> <\"a\"> <\"b\"> <<<<dq(\"[\" $(list (cmd <\"cat\"> <<<\"EOF\">[\"body\\n\"])) \"]\")>) (cmd <\"echo\"> <\"after\">))"
    );
    let _ = "".to_string();
}

#[test]
fn deep_nesting_fits_a_small_stack() {
    extern crate std;
    // wasm32 gives a 1 MiB stack by default. Debug frames are several times larger
    // than release ones, so debug builds get 4 MiB.
    let patterns = [
        "( ",
        "{ ",
        "$(",
        "\"$(",
        "${x:-",
        "$((",
        "if ",
        "[[ ( ",
        "a=(",
        "<(",
        "`",
        "case x in x) ",
    ];
    let handle = std::thread::Builder::new()
        .stack_size(if cfg!(debug_assertions) {
            4 << 20
        } else {
            1 << 20
        })
        .spawn(move || {
            for p in patterns {
                let src = format!("echo {}", p.repeat(1000));
                let _ = parse(&src);
            }
        })
        .unwrap();
    handle.join().unwrap();
}

#[test]
fn bytes_that_are_not_utf8() {
    let src: &[u8] = b"echo a\xffb \"\xfe$x\" '\xc3' \\\xff; cat <<E\xff\nbody \xfe $y\nE\xff\n# \xff comment\nf\xff() { :; }\n";
    let ast = parse_bytes(src, Options::default()).unwrap();
    assert!(!ast.is_utf8());
    assert_eq!(ast.source_bytes(), src);
    check_spans(&ast);
    let items = &ast.list(ast.root()).items;
    assert_eq!(items.len(), 3);
    let CommandKind::Simple(s) = &ast.cmd(items[0].first.stages[0].cmd).kind else {
        panic!()
    };
    // Spans are offsets into the original bytes; the text view shows U+F7xx.
    assert_eq!(s.words[1].span, Span::new(5, 8));
    assert_eq!(ast.bytes(s.words[1].span), b"a\xffb");
    assert_eq!(ast.text(s.words[1].span), "a\u{f7ff}b");
    assert_eq!(ast.bytes(s.words[4].span), b"\\\xff");
    assert!(matches!(s.words[4].parts[0].kind, Part::Escaped));
    // The here-doc delimiter matches byte for byte.
    let h = ast.heredoc(HereDocId(0));
    assert!(h.terminated);
    assert_eq!(ast.bytes(h.body.span), b"body \xfe $y\n");
    // Printing goes through the view; restoring gives the bytes back.
    let printed = print(&ast);
    let again = parse_bytes(&ast.restore_bytes(&printed), Options::default()).unwrap();
    assert_eq!(dump(&again), dump(&ast));
    assert_eq!(&*ast.restore_bytes(ast.text(s.words[1].span)), b"a\xffb");
    // Columns count an undecodable byte as one.
    let idx = LineIndex::from_bytes(src);
    let lc = idx.line_col_bytes(src, 8);
    assert_eq!((lc.line, lc.column), (1, 9));
    // Incomplete sequences stand alone and never swallow the next char.
    let ast = parse_bytes(b"echo \xe2\x82; echo \xc3", Options::default()).unwrap();
    assert_eq!(ast.list(ast.root()).items.len(), 2);
    assert_eq!(
        parse_bytes(b"echo \xc3\"", Options::default())
            .unwrap_err()
            .message,
        "unexpected EOF while looking for matching `\"'"
    );
}

#[test]
fn reserved_words_and_subscripts_found_by_fuzzing() {
    // Checked against bash 5.3 (Linux, C.UTF-8).
    assert_eq!(err("in"), "syntax error near unexpected token `in'");
    assert_eq!(err("]]"), "syntax error near unexpected token `]]'");
    assert_eq!(err("a[b"), "unexpected EOF while looking for matching `]'");
    assert_eq!(
        err("x=1 a["),
        "unexpected EOF while looking for matching `]'"
    );
    d("echo a[");
    d("a[]");
    d("echo ${ :; }x ${ } ${${x}} ${$(echo)} ${$}");
    assert_eq!(
        err("{\\\n"),
        "syntax error: unexpected end of file from `{' command on line 1"
    );
    d("<<then case\nthen");
}

/// Parses `src` chunk by chunk with `aliases`; the dumps of the chunks, and the asts.
fn chunks_with(src: &str, aliases: &[(&str, &str)]) -> (Vec<String>, Vec<Ast>) {
    let mut table = Aliases::new();
    for (n, v) in aliases {
        table.set(n.as_bytes(), v.as_bytes());
    }
    let mut p = Parser::new(src, Options::default());
    p.set_aliases(Some(&table));
    let mut dumps = Vec::new();
    let mut asts = Vec::new();
    while let Some(chunk) = p.next_chunk() {
        let ast = chunk.unwrap_or_else(|e| panic!("{src:?}: {e}"));
        check_spans_alias(&ast);
        dumps.push(dump(&ast));
        asts.push(ast);
    }
    (dumps, asts)
}

/// Like `check_spans`, for chunks with alias text: every span lies in the script or in
/// one alias text, and children lie in their parents.
fn check_spans_alias(ast: &Ast) {
    let all = Span::new(0, ast.source_bytes().len() as u32);
    let mut w = Walk {
        ast,
        lines: Vec::new(),
    };
    w.list(ast.root(), all);
    for line in &w.lines {
        let _ = line;
    }
}

#[test]
fn aliases_expand_while_reading() {
    let (d, _) = chunks_with(
        "ll x\necho ll\n'll' x\n\\ll x\nll\"\" x",
        &[("ll", "ls -l")],
    );
    assert_eq!(
        d,
        [
            "(list (cmd <\"ls\"> <\"-l\"> <\"x\">))",
            "(list (cmd <\"echo\"> <\"ll\">))",
            "(list (cmd <'ll'> <\"x\">))",
            "(list (cmd <\\l \"l\"> <\"x\">))",
            "(list (cmd <\"ll\" dq()> <\"x\">))",
        ]
    );
    // Before reserved words, after `!`, `time`, assignments, redirections, operators,
    // and inside `$(…)`.
    let (d, _) = chunks_with(
        "myif e; then e a; fi\n! e b | e c && A=1 e d; >o e f; time e g; x=$(e h)",
        &[("myif", "if"), ("e", "echo")],
    );
    assert_eq!(
        d[0],
        "(list (if (list (cmd <\"echo\">)) (list (cmd <\"echo\"> <\"a\">))))"
    );
    assert_eq!(
        d[1],
        "(list (andor (pipe ! (cmd <\"echo\"> <\"b\">) | (cmd <\"echo\"> <\"c\">)) && (cmd (assign A= <\"1\">) <\"echo\"> <\"d\">)) (cmd <\"echo\"> <\"f\"> ><\"o\">) (pipe time (cmd <\"echo\"> <\"g\">)) (cmd (assign x= <$(list (cmd <\"echo\"> <\"h\">))>)))"
    );
    // A trailing blank makes the next word a candidate; no re-expansion of an alias
    // inside itself; an alias text may hold several commands and a newline.
    let (d, _) = chunks_with(
        "a b c\nls x\nx\nt foo\nm; echo 3",
        &[
            ("a", "echo "),
            ("b", "B "),
            ("c", "C"),
            ("ls", "ls -l"),
            ("x", "y"),
            ("y", "x"),
            ("e", "echo"),
            ("t", "e e"),
            ("m", "echo 1\necho 2"),
        ],
    );
    assert_eq!(
        d,
        [
            "(list (cmd <\"echo\"> <\"B\"> <\"C\">))",
            "(list (cmd <\"ls\"> <\"-l\"> <\"x\">))",
            "(list (cmd <\"x\">))",
            "(list (cmd <\"echo\"> <\"e\"> <\"foo\">))",
            "(list (cmd <\"echo\"> <\"1\">) (cmd <\"echo\"> <\"2\">) (cmd <\"echo\"> <\"3\">))",
        ]
    );
    // Keywords and operators from alias text.
    let (d, _) = chunks_with(
        "c x in x) e m;; esac\nb e x; }",
        &[("c", "case"), ("e", "echo"), ("b", "{")],
    );
    assert_eq!(
        d[0],
        "(list (case <\"x\"> (<\"x\">) (list (cmd <\"echo\"> <\"m\">)) ;;))"
    );
    assert_eq!(d[1], "(list (brace (list (cmd <\"echo\"> <\"x\">))))");
    // An alias defined on a line is not in effect on that line: the table is what the
    // shell passes per chunk, so this is the engine's part. Without a table nothing
    // expands.
    let mut p = Parser::new("ll x", Options::default());
    p.set_aliases(None);
    assert_eq!(
        dump(&p.next_chunk().unwrap().unwrap()),
        "(list (cmd <\"ll\"> <\"x\">))"
    );
}

#[test]
fn alias_spans() {
    let src = "echo a; ll x y\n";
    let (_, asts) = chunks_with(src, &[("ll", "ls -l")]);
    let ast = &asts[0];
    assert_eq!(ast.script_len(), src.len() as u32);
    let uses = ast.alias_uses();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].name, b"ll");
    assert_eq!(uses[0].word, Span::new(8, 10));
    assert_eq!(ast.text(uses[0].text), "ls -l");
    let item = &ast.list(ast.root()).items[1];
    let cmd = ast.cmd(item.first.stages[0].cmd);
    // The command covers the alias word and the script text after it.
    assert_eq!(cmd.span, Span::new(8, 14));
    let CommandKind::Simple(s) = &cmd.kind else {
        panic!()
    };
    // Words from the alias point into its text, after the script.
    assert!(s.words[0].span.start >= ast.script_len());
    assert_eq!(ast.text(s.words[1].span), "-l");
    assert_eq!(ast.original_span(s.words[1].span), Span::new(8, 10));
    assert_eq!(ast.text(s.words[2].span), "x");
    assert_eq!(ast.original_span(s.words[2].span), s.words[2].span);
    // The second chunk starts after the line.
    assert_eq!(asts.len(), 1);
    // Nested: an alias text that starts with another alias.
    let (_, asts) = chunks_with("t foo\n", &[("e", "echo"), ("t", "e e")]);
    let ast = &asts[0];
    assert_eq!(ast.alias_uses().len(), 2);
    let cmd = ast.cmd(ast.list(ast.root()).items[0].first.stages[0].cmd);
    assert_eq!(cmd.span, Span::new(0, 5));
    // An error in alias text points at the alias word.
    let mut table = Aliases::new();
    table.set(b"bad", b"echo )");
    let mut p = Parser::new("echo ok\nbad x\n", Options::default());
    p.set_aliases(Some(&table));
    assert!(p.next_chunk().unwrap().is_ok());
    let e = p.next_chunk().unwrap().unwrap_err();
    assert_eq!(e.message, "syntax error near unexpected token `)'");
    assert_eq!(e.span, Span::new(8, 11));
}

#[test]
fn alias_corners_found_by_fuzzing() {
    // Each checked against bash 5.3 with a toy runner (examples/aliasrun.rs).
    // An alias with empty or blank text is as if the word were not there.
    let (d, _) = chunks_with(
        "e x\ntrue && e\nfalse\ntrue; s\necho z",
        &[("e", ""), ("s", " "), ("x", "echo X")],
    );
    assert_eq!(
        d,
        [
            "(list (cmd <\"echo\"> <\"X\">))",
            "(list (andor (cmd <\"true\">) && (cmd <\"false\">)))",
            "(list (cmd <\"true\">))",
            "(list (cmd <\"echo\"> <\"z\">))",
        ]
    );
    let mut table = Aliases::new();
    table.set(b"q", b"");
    let mut p = Parser::new("l || q; echo", Options::default());
    p.set_aliases(Some(&table));
    assert_eq!(
        p.next_chunk().unwrap().unwrap_err().message,
        "syntax error near unexpected token `;'"
    );
    // A comment in alias text runs on into the line.
    let (d, _) = chunks_with("c there\necho next", &[("c", "echo hi # x")]);
    assert_eq!(
        d,
        [
            "(list (cmd <\"echo\"> <\"hi\">))",
            "(list (cmd <\"echo\"> <\"next\">))"
        ]
    );
    // `!` from an alias, then `&&`: an error, as `! && x` is.
    table.set(b"a", b"!");
    let mut p = Parser::new("a && echo", Options::default());
    p.set_aliases(Some(&table));
    assert_eq!(
        p.next_chunk().unwrap().unwrap_err().message,
        "syntax error near unexpected token `&&'"
    );
}
