//! `psh --parse`: the syntax tree as JSON.
//!
//! The shape, for the classifier, UIs and tests (every node is an object with `type` and
//! `span`; spans are `[start, end]` byte offsets into the UTF-8 source):
//!
//! - Result: `{"ok": true, "script": LIST}` (with `"utf8": false` when the input is not
//!   UTF-8: texts then show each undecodable byte as U+F780..U+F7FF) or `{"ok": false, "error": {"message",
//!   "span", "line", "column", "incomplete"}}` (`line`/`column` 1-based, column in chars;
//!   `message` may have several lines, as bash prints them).
//! - `list`: `items` (and-or lists). `and_or`: `pipelines`, `operators` (`&&`/`||`
//!   between them), `background`. `pipeline`: `commands`, `pipes` (`|`/`|&` between
//!   them), `negated`, `bangs`, `time` (`null` or `{span, posix}`).
//! - Commands, all with `redirects`: `simple` (`assigns`, `words`), `brace`/`subshell`
//!   (`body`), `if` (`branches: [{span, cond, body}]`, `else`), `while`/`until` (`cond`,
//!   `body`), `for`/`select` (`var`, `words` or `null` for `"$@"`, `body`), `arith_for`
//!   (`init`, `cond`, `step`, `body`), `case` (`word`, `items: [{span, patterns, body,
//!   terminator}]`), `arith` (`expr`), `cond` (`expr`), `function` (`name`, `keyword`,
//!   `body`), `coproc` (`name`, `body`).
//! - `word`: `text` as written, `value` (the word after quote removal if it has no
//!   expansions, else `null`), `quoted` (any part quoted), `parts`.
//! - Parts: `lit`/`escaped`/`single`/`ansi_c` with `text` (the literal value; `ansi_c`
//!   keeps its escapes), `double`/`locale` with `parts`, `param`, `bad_subst`,
//!   `cmd_subst`/`funsub`/`proc_subst` with `body` (a list), `backtick` (`text`: the
//!   unescaped body; `body`: its parse, whose spans are offsets into `text`, or `null`
//!   and `error`), `arith` (`expr`: a word), `brace` (`alts`) and `brace_seq` (`start`,
//!   `end`, `step`), `extglob` (`op`, `alts`), `array` (`elems`).
//! - `param`: `name`, `name_kind` (`var`, `positional`, `special`), `braced`, `prefix`
//!   (`null`, `#` length, `!` indirect), `index` (`null`, `"@"`, `"*"` or a word), `op`
//!   (`null` or the operator as written: `:-`, `#`, `//`, `^^`, `:`, `@`, `names`, …)
//!   with its operands (`word`, `pattern`, `replacement`, `offset`, `length`,
//!   `transform`, `star`).
//! - `assign`: `name`, `index`, `append`, `value` (a word or an `array`). `array`:
//!   `elems: [{span, key, append, value}]`.
//! - `redirect`: `fd` (number or `null`), `fd_var` (`{name}` form), `op` as written, and
//!   `target` (a word) or `heredoc` (`delimiter`, `quoted`, `strip_tabs`, `body`,
//!   `terminated`, `error`).
//! - `[[ ]]` expressions: `cond_unary` (`op`, `arg`), `cond_binary` (`op`, `lhs`,
//!   `rhs`), `cond_word` (`word`), `cond_not` (`expr`), `cond_and`/`cond_or` (`left`,
//!   `right`), `cond_group` (`expr`).

use psh_syntax::*;

use crate::json::Json;

/// Parses `source` and returns the JSON result and whether it parsed.
pub fn parse_to_json(source: &[u8], opts: Options) -> (Json, bool) {
    match parse_bytes(source, opts) {
        Ok(ast) => {
            let script = Conv::new(&ast).list(ast.root());
            let mut j = Json::obj().set("ok", true);
            if !ast.is_utf8() {
                // Texts show undecodable bytes as U+F780..U+F7FF; spans stay exact.
                j = j.set("utf8", false);
            }
            (j.set("script", script), true)
        }
        Err(e) => (error_json(source, &e), false),
    }
}

/// The outline of `psh_syntax::outline` (the docs' AST tab): `{"ok": true, "rows":
/// [{depth, role, kind, info, text, span}]}`, or the error as `parse_to_json` gives it.
#[allow(dead_code)] // psh prints the outline as text; psh-wasm and psh-docs use this.
pub fn outline_to_json(source: &[u8], opts: Options) -> (Json, bool) {
    match parse_bytes(source, opts) {
        Ok(ast) => {
            let rows = outline(&ast)
                .into_iter()
                .map(|r| {
                    Json::obj()
                        .set("depth", r.depth)
                        .set("role", r.role)
                        .set("kind", r.kind)
                        .set("info", r.info)
                        .set("text", r.text)
                        .set("span", span(r.span))
                })
                .collect::<Vec<_>>();
            (Json::obj().set("ok", true).set("rows", rows), true)
        }
        Err(e) => (error_json(source, &e), false),
    }
}

fn error_json(source: &[u8], e: &ParseError) -> Json {
    let lc = LineIndex::from_bytes(source).line_col_bytes(source, e.span.start);
    let error = Json::obj()
        .set("message", e.message.as_str())
        .set("span", span(e.span))
        .set("line", lc.line)
        .set("column", lc.column)
        .set("incomplete", e.incomplete);
    Json::obj().set("ok", false).set("error", error)
}

fn span(s: Span) -> Json {
    Json::Arr(vec![Json::from(s.start), Json::from(s.end)])
}

fn node(kind: &str, s: Span) -> Json {
    Json::obj().set("type", kind).set("span", span(s))
}

struct Conv<'a> {
    ast: &'a Ast,
    /// Inside double quotes, where a backquoted body unescapes `\"` too.
    in_double: bool,
}

impl<'a> Conv<'a> {
    fn new(ast: &'a Ast) -> Conv<'a> {
        Conv {
            ast,
            in_double: false,
        }
    }

    fn text(&self, s: Span) -> &'a str {
        self.ast.text(s)
    }

    fn list(&mut self, id: ListId) -> Json {
        let l = self.ast.list(id);
        let items = l.items.iter().map(|ao| self.and_or(ao)).collect::<Vec<_>>();
        node("list", l.span).set("items", items)
    }

    fn and_or(&mut self, ao: &AndOr) -> Json {
        let mut pipelines = vec![self.pipeline(&ao.first)];
        let mut ops = Vec::new();
        for (op, p) in &ao.rest {
            ops.push(Json::from(match op {
                AndOrOp::And => "&&",
                AndOrOp::Or => "||",
            }));
            pipelines.push(self.pipeline(p));
        }
        node("and_or", ao.span)
            .set("pipelines", pipelines)
            .set("operators", ops)
            .set("background", ao.background)
    }

    fn pipeline(&mut self, p: &Pipeline) -> Json {
        let commands = p.stages.iter().map(|s| self.cmd(s.cmd)).collect::<Vec<_>>();
        let pipes = p
            .stages
            .iter()
            .take(p.stages.len().saturating_sub(1))
            .map(|s| Json::from(if s.pipe_stderr { "|&" } else { "|" }))
            .collect::<Vec<_>>();
        let time = p
            .time
            .map(|t| Json::obj().set("span", span(t.span)).set("posix", t.posix));
        node("pipeline", p.span)
            .set("commands", commands)
            .set("pipes", pipes)
            .set("negated", p.negated())
            .set("bangs", p.bangs)
            .set("time", time)
    }

    fn opt_list(&mut self, id: Option<ListId>) -> Json {
        id.map_or(Json::Null, |l| self.list(l))
    }

    fn cmd(&mut self, id: CmdId) -> Json {
        let ast = self.ast;
        let c = ast.cmd(id);
        let j = match &c.kind {
            CommandKind::Simple(s) => {
                let assigns = s.assigns.iter().map(|a| self.assign(a)).collect::<Vec<_>>();
                let words = self.words(&s.words);
                node("simple", c.span)
                    .set("assigns", assigns)
                    .set("words", words)
            }
            CommandKind::Brace(l) => node("brace", c.span).set("body", self.list(*l)),
            CommandKind::Subshell(l) => node("subshell", c.span).set("body", self.list(*l)),
            CommandKind::If(i) => {
                let branches = i
                    .branches
                    .iter()
                    .map(|b| {
                        Json::obj()
                            .set("span", span(b.span))
                            .set("cond", self.list(b.cond))
                            .set("body", self.list(b.body))
                    })
                    .collect::<Vec<_>>();
                node("if", c.span)
                    .set("branches", branches)
                    .set("else", self.opt_list(i.else_body))
            }
            CommandKind::While(w) => node(if w.until { "until" } else { "while" }, c.span)
                .set("cond", self.list(w.cond))
                .set("body", self.list(w.body)),
            CommandKind::For(f) | CommandKind::Select(f) => {
                let kind = if matches!(c.kind, CommandKind::For(_)) {
                    "for"
                } else {
                    "select"
                };
                let words = f.words.as_ref().map(|w| self.words(w));
                node(kind, c.span)
                    .set("var", self.word(&f.var))
                    .set("words", words)
                    .set("body", self.list(f.body))
            }
            CommandKind::ArithFor(f) => node("arith_for", c.span)
                .set("init", self.word(&f.init))
                .set("cond", self.word(&f.cond))
                .set("step", self.word(&f.step))
                .set("body", self.list(f.body)),
            CommandKind::Case(cs) => {
                let items = cs
                    .items
                    .iter()
                    .map(|item| {
                        let patterns = self.words(&item.patterns);
                        Json::obj()
                            .set("span", span(item.span))
                            .set("patterns", patterns)
                            .set("body", self.opt_list(item.body))
                            .set(
                                "terminator",
                                match item.terminator {
                                    CaseTerminator::Break => ";;",
                                    CaseTerminator::FallThrough => ";&",
                                    CaseTerminator::Continue => ";;&",
                                },
                            )
                    })
                    .collect::<Vec<_>>();
                node("case", c.span)
                    .set("word", self.word(&cs.word))
                    .set("items", items)
            }
            CommandKind::Arith(w) => node("arith", c.span).set("expr", self.word(w)),
            CommandKind::Cond(e) => node("cond", c.span).set("expr", self.cond(e)),
            CommandKind::Function(f) => node("function", c.span)
                .set("name", self.word(&f.name))
                .set("keyword", f.keyword)
                .set("body", self.cmd(f.body)),
            CommandKind::Coproc(cp) => {
                let name = cp.name.as_ref().map(|n| self.word(n));
                node("coproc", c.span)
                    .set("name", name)
                    .set("body", self.cmd(cp.body))
            }
        };
        let redirects = c
            .redirects
            .iter()
            .map(|r| self.redirect(r))
            .collect::<Vec<_>>();
        j.set("redirects", redirects)
    }

    fn redirect(&mut self, r: &Redirect) -> Json {
        let (fd, fd_var) = match r.fd {
            Some(RedirFd::Number(n)) => (Json::from(n), Json::Null),
            Some(RedirFd::Var(s)) => (Json::Null, Json::from(self.text(s))),
            None => (Json::Null, Json::Null),
        };
        let j = node("redirect", r.span)
            .set("fd", fd)
            .set("fd_var", fd_var)
            .set("op", r.op.as_str());
        match &r.target {
            RedirTarget::Word(w) => j.set("target", self.word(w)).set("heredoc", Json::Null),
            RedirTarget::HereDoc(id) => {
                let h = self.ast.heredoc(*id);
                let heredoc = Json::obj()
                    .set("delimiter", self.word(&h.delimiter))
                    .set("quoted", h.quoted)
                    .set("strip_tabs", h.strip_tabs)
                    .set("body", self.word(&h.body))
                    .set("terminated", h.terminated)
                    .set("error", h.error.as_ref().map(|e| e.message.as_str()));
                j.set("target", Json::Null).set("heredoc", heredoc)
            }
        }
    }

    fn assign(&mut self, a: &Assign) -> Json {
        let index = a.index.as_ref().map(|w| self.word(w));
        let value = match &a.value {
            AssignValue::Scalar(w) => self.word(w),
            AssignValue::Array(arr) => self.array(arr),
        };
        node("assign", a.span)
            .set("name", self.text(a.name))
            .set("index", index)
            .set("append", a.append)
            .set("value", value)
    }

    fn array(&mut self, arr: &ArrayLit) -> Json {
        let elems = arr
            .elems
            .iter()
            .map(|e| {
                let key = e.key.as_ref().map(|k| self.word(k));
                Json::obj()
                    .set("span", span(e.span))
                    .set("key", key)
                    .set("append", e.append)
                    .set("value", self.word(&e.value))
            })
            .collect::<Vec<_>>();
        node("array", arr.span).set("elems", elems)
    }

    fn cond(&mut self, e: &CondExpr) -> Json {
        match &e.kind {
            CondKind::Unary { op_span, arg, .. } => node("cond_unary", e.span)
                .set("op", self.text(*op_span))
                .set("op_span", span(*op_span))
                .set("arg", self.word(arg)),
            CondKind::Binary {
                op_span, lhs, rhs, ..
            } => node("cond_binary", e.span)
                .set("op", self.text(*op_span))
                .set("op_span", span(*op_span))
                .set("lhs", self.word(lhs))
                .set("rhs", self.word(rhs)),
            CondKind::Word(w) => node("cond_word", e.span).set("word", self.word(w)),
            CondKind::Not(i) => node("cond_not", e.span).set("expr", self.cond(i)),
            CondKind::Group(i) => node("cond_group", e.span).set("expr", self.cond(i)),
            CondKind::And(a, b) => node("cond_and", e.span)
                .set("left", self.cond(a))
                .set("right", self.cond(b)),
            CondKind::Or(a, b) => node("cond_or", e.span)
                .set("left", self.cond(a))
                .set("right", self.cond(b)),
        }
    }

    fn words(&mut self, words: &[Word]) -> Vec<Json> {
        words.iter().map(|w| self.word(w)).collect()
    }

    fn word(&mut self, w: &Word) -> Json {
        let quoted = w.parts.iter().any(|p| {
            matches!(
                p.kind,
                Part::Escaped | Part::Single | Part::AnsiC | Part::Double(_) | Part::Locale(_)
            )
        });
        node("word", w.span)
            .set("text", self.text(w.span))
            .set("value", literal_text(self.ast, w))
            .set("quoted", quoted)
            .set("parts", self.parts(&w.parts))
    }

    fn parts(&mut self, parts: &[WordPart]) -> Vec<Json> {
        parts.iter().map(|p| self.part(p)).collect()
    }

    fn part(&mut self, p: &WordPart) -> Json {
        let text = self.text(p.span);
        let sp = p.span;
        match &p.kind {
            Part::Lit => node("lit", sp).set("text", text),
            Part::Escaped => node("escaped", sp).set("text", &text[1..]),
            Part::Single => node("single", sp).set("text", &text[1..text.len() - 1]),
            Part::AnsiC => node("ansi_c", sp).set("text", &text[2..text.len() - 1]),
            Part::Double(inner) | Part::Locale(inner) => {
                let kind = if matches!(p.kind, Part::Double(_)) {
                    "double"
                } else {
                    "locale"
                };
                let outer = std::mem::replace(&mut self.in_double, true);
                let parts = self.parts(inner);
                self.in_double = outer;
                node(kind, sp).set("parts", parts)
            }
            Part::Param(pe) => self.param(pe, sp),
            Part::BadSubst => node("bad_subst", sp).set("text", text),
            Part::CmdSubst(l) => {
                let outer = std::mem::replace(&mut self.in_double, false);
                let body = self.list(*l);
                self.in_double = outer;
                node("cmd_subst", sp).set("body", body)
            }
            Part::FunSub { list, reply } => {
                let outer = std::mem::replace(&mut self.in_double, false);
                let body = self.list(*list);
                self.in_double = outer;
                node("funsub", sp).set("reply", *reply).set("body", body)
            }
            Part::ProcSubst { output, list } => {
                let body = self.list(*list);
                node("proc_subst", sp)
                    .set("direction", if *output { ">" } else { "<" })
                    .set("body", body)
            }
            Part::Backtick => {
                let inner = backtick_body(text, self.in_double);
                let (body, error) = match parse(&inner) {
                    Ok(ast) => (Conv::new(&ast).list(ast.root()), Json::Null),
                    Err(e) => (Json::Null, Json::from(e.message)),
                };
                node("backtick", sp)
                    .set("text", inner)
                    .set("body", body)
                    .set("error", error)
            }
            Part::Arith(w) => node("arith", sp).set("expr", self.word(w)),
            Part::Brace(b) => match &**b {
                Brace::Alts(alts) => node("brace", sp).set("alts", self.words(alts)),
                Brace::Seq { start, end, step } => node("brace_seq", sp)
                    .set("start", self.text(*start))
                    .set("end", self.text(*end))
                    .set("step", step.map(|s| self.text(s))),
            },
            Part::ExtGlob { op, alts } => node("extglob", sp)
                .set("op", (*op as char).to_string())
                .set("alts", self.words(alts)),
            Part::Array(arr) => self.array(arr),
        }
    }

    fn param(&mut self, pe: &Param, sp: Span) -> Json {
        let name_kind = match pe.name {
            ParamName::Var => "var",
            ParamName::Positional => "positional",
            ParamName::Special(_) => "special",
        };
        let prefix = match pe.prefix {
            ParamPrefix::None => Json::Null,
            ParamPrefix::Length => Json::from("#"),
            ParamPrefix::Indirect => Json::from("!"),
        };
        let index = match &pe.index {
            None => Json::Null,
            Some(Subscript::At) => Json::from("@"),
            Some(Subscript::Star) => Json::from("*"),
            Some(Subscript::Expr(w)) => self.word(w),
        };
        let j = node("param", sp)
            .set("name", self.text(pe.name_span))
            .set("name_kind", name_kind)
            .set("name_span", span(pe.name_span))
            .set("braced", pe.braced)
            .set("prefix", prefix)
            .set("index", index);
        match &pe.op {
            ParamOp::None => j.set("op", Json::Null),
            ParamOp::Default { kind, colon, word } => {
                let op = match kind {
                    DefaultKind::Use => "-",
                    DefaultKind::Assign => "=",
                    DefaultKind::Error => "?",
                    DefaultKind::Alternative => "+",
                };
                let op = if *colon {
                    format!(":{op}")
                } else {
                    op.to_string()
                };
                j.set("op", op).set("word", self.word(word))
            }
            ParamOp::RemovePrefix { longest, pattern } => j
                .set("op", if *longest { "##" } else { "#" })
                .set("pattern", self.word(pattern)),
            ParamOp::RemoveSuffix { longest, pattern } => j
                .set("op", if *longest { "%%" } else { "%" })
                .set("pattern", self.word(pattern)),
            ParamOp::Replace {
                kind,
                pattern,
                replacement,
            } => {
                let op = match kind {
                    ReplaceKind::First => "/",
                    ReplaceKind::All => "//",
                    ReplaceKind::Prefix => "/#",
                    ReplaceKind::Suffix => "/%",
                };
                let replacement = replacement.as_ref().map(|r| self.word(r));
                j.set("op", op)
                    .set("pattern", self.word(pattern))
                    .set("replacement", replacement)
            }
            ParamOp::Case { kind, pattern } => {
                let op = match kind {
                    CaseKind::UpperFirst => "^",
                    CaseKind::UpperAll => "^^",
                    CaseKind::LowerFirst => ",",
                    CaseKind::LowerAll => ",,",
                    CaseKind::ToggleFirst => "~",
                    CaseKind::ToggleAll => "~~",
                };
                let pattern = pattern.as_ref().map(|p| self.word(p));
                j.set("op", op).set("pattern", pattern)
            }
            ParamOp::Substring { offset, length } => {
                let length = length.as_ref().map(|l| self.word(l));
                j.set("op", ":")
                    .set("offset", self.word(offset))
                    .set("length", length)
            }
            ParamOp::Transform(c) => j.set("op", "@").set("transform", (*c as char).to_string()),
            ParamOp::Names { star } => j.set("op", "names").set("star", *star),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(src: &str) -> Json {
        let (j, ok) = parse_to_json(src.as_bytes(), Options::default());
        assert!(ok, "{}", j.to_pretty());
        j
    }

    #[test]
    fn simple_command() {
        let j = json("echo \"a $b\" >out");
        assert_eq!(
            j.to_compact(),
            concat!(
                r#"{"ok":true,"script":{"type":"list","span":[0,16],"items":[{"type":"and_or","span":[0,16],"pipelines":[{"type":"pipeline","span":[0,16],"commands":[{"type":"simple","span":[0,16],"assigns":[],"words":["#,
                r#"{"type":"word","span":[0,4],"text":"echo","value":"echo","quoted":false,"parts":[{"type":"lit","span":[0,4],"text":"echo"}]},"#,
                r#"{"type":"word","span":[5,11],"text":"\"a $b\"","value":null,"quoted":true,"parts":[{"type":"double","span":[5,11],"parts":[{"type":"lit","span":[6,8],"text":"a "},{"type":"param","span":[8,10],"name":"b","name_kind":"var","name_span":[9,10],"braced":false,"prefix":null,"index":null,"op":null}]}]}],"#,
                r#""redirects":[{"type":"redirect","span":[12,16],"fd":null,"fd_var":null,"op":">","target":{"type":"word","span":[13,16],"text":"out","value":"out","quoted":false,"parts":[{"type":"lit","span":[13,16],"text":"out"}]},"heredoc":null}]}],"#,
                r#""pipes":[],"negated":false,"bangs":0,"time":null}],"operators":[],"background":false}]}}"#
            )
        );
    }

    #[test]
    fn error() {
        let (j, ok) = parse_to_json(b"echo a\nfi", Options::default());
        assert!(!ok);
        assert_eq!(
            j.to_compact(),
            r#"{"ok":false,"error":{"message":"syntax error near unexpected token `fi'","span":[7,9],"line":2,"column":1,"incomplete":false}}"#
        );
    }

    #[test]
    fn nested_substitutions_and_backticks() {
        let j = json("a=`echo \\`date\\``; b=\"`echo \\\"x\\\"`\"");
        let text = j.to_compact();
        assert!(
            text.contains(r#""type":"backtick","span":[2,17],"text":"echo `date`""#),
            "{text}"
        );
        assert!(text.contains(r#""text":"echo \"x\"""#), "{text}");
        let j = json("x=$(ls <(cat) | wc -l)");
        let text = j.to_compact();
        assert!(text.contains(r#""type":"cmd_subst""#));
        assert!(text.contains(r#""type":"proc_subst","span":[7,13],"direction":"<""#));
    }

    #[test]
    fn outline_rows() {
        let (j, ok) = outline_to_json(b"echo $x", Options::default());
        assert!(ok);
        assert_eq!(
            j.to_compact(),
            concat!(
                r#"{"ok":true,"rows":[{"depth":0,"role":"","kind":"simple","info":"","text":"echo $x","span":[0,7]},"#,
                r#"{"depth":1,"role":"","kind":"word","info":"","text":"echo","span":[0,4]},"#,
                r#"{"depth":1,"role":"","kind":"param","info":"x","text":"$x","span":[5,7]}]}"#
            )
        );
        let (j, ok) = outline_to_json(b"if", Options::default());
        assert!(!ok);
        assert!(j.to_compact().starts_with(r#"{"ok":false,"error":{"#));
    }

    #[test]
    fn every_kind_converts() {
        // Smoke test over every construct: all of it must serialize.
        let src = "A=1 b[2]+=(x [k]=v) cmd 'l' $'c' $\"t\" \\e ${v:-w} ${#v} ${a[i]} ${x//a/b} \
                   ${x:1:2} ${x^^} ${x@Q} ${!p*} ${ f; } $((1+2)) {x,y} {1..2} ${x!} 2>&1 {fd}<i <<'H'\n\
                   body\nH\n\
                   time ! a | b |& c && d || e &\n\
                   if i; then j; else m; fi; while n; do o; done; until p; do q; done\n\
                   for r in s; do t; done; select u; do w; done; for ((;;)); do y; done\n\
                   case z in a|b) c;; *) ;& esac; { d; }; (e); ((f)); [[ -f g && ( h == i || ! j ) && k ]]\n\
                   fn() { k; }; function g { :; }; coproc N { l; }; declare -a arr=(m)";
        let j = json(src);
        let pretty = j.to_pretty();
        for kind in [
            "\"simple\"",
            "\"brace\"",
            "\"subshell\"",
            "\"if\"",
            "\"while\"",
            "\"until\"",
            "\"for\"",
            "\"select\"",
            "\"arith_for\"",
            "\"case\"",
            "\"arith\"",
            "\"cond\"",
            "\"function\"",
            "\"coproc\"",
            "\"cond_unary\"",
            "\"cond_binary\"",
            "\"cond_word\"",
            "\"cond_not\"",
            "\"cond_and\"",
            "\"cond_or\"",
            "\"cond_group\"",
            "\"funsub\"",
            "\"bad_subst\"",
            "\"brace_seq\"",
            "\"array\"",
            "\"assign\"",
            "\"redirect\"",
        ] {
            assert!(pretty.contains(&format!("\"type\": {kind}")), "{kind}");
        }
    }
}
