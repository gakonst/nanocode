//! Recursive-descent parser for the builtin subset (builtins/*.ts) and for the intrinsic
//! declarations (intrinsics.d.ts). It accepts only the subset: anything else is an error
//! that says what to use instead.

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{Tok, Token, tokenize};

type PResult<T> = Result<T, Diagnostic>;

const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "await",
    "async",
    "undefined",
    "arguments",
    "eval",
];

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
}

pub fn parse_module(file: u32, src: &str) -> PResult<Module> {
    let toks = tokenize(file, src)?;
    let mut p = Parser {
        toks: &toks,
        pos: 0,
    };
    let mut functions = Vec::new();
    while !p.at_eof() {
        functions.push(p.function()?);
    }
    Ok(Module { file, functions })
}

pub fn parse_declarations(file: u32, src: &str) -> PResult<Declarations> {
    let toks = tokenize(file, src)?;
    let mut p = Parser {
        toks: &toks,
        pos: 0,
    };
    let mut intrinsics = Vec::new();
    while !p.at_eof() {
        if p.is_ident("type") {
            // Type aliases are for tsc; bgen's types are fixed (ast::Type).
            p.skip_to_semicolon()?;
            continue;
        }
        let doc = p.peek().doc.clone();
        let start = p.peek().span;
        p.expect_ident("declare")?;
        p.expect_ident("function")?;
        let (name, _) = p.ident()?;
        let params = p.params()?.1;
        p.expect(":")?;
        let ret = p.type_ref()?;
        let end = p.expect(";")?;
        intrinsics.push(IntrinsicDecl {
            name,
            params,
            ret,
            tags: parse_tags(doc.as_deref()),
            span: start.to(end),
        });
    }
    Ok(Declarations { intrinsics })
}

/// The `@tags` of a JSDoc comment. A tag starts with `@` at the start of a line or
/// after whitespace and runs to the next tag or the end of the line, so one-line
/// comments like `/** Description. @leaf */` work. Everything else is description.
pub fn parse_tags(doc: Option<&str>) -> Vec<Tag> {
    let mut tags = Vec::new();
    for line in doc.unwrap_or("").lines() {
        let line = line.trim().trim_start_matches('*').trim();
        let starts: Vec<usize> = line
            .char_indices()
            .filter(|&(i, c)| c == '@' && (i == 0 || line[..i].ends_with(char::is_whitespace)))
            .map(|(i, _)| i)
            .collect();
        for (n, &start) in starts.iter().enumerate() {
            let end = starts.get(n + 1).copied().unwrap_or(line.len());
            let rest = &line[start + 1..end];
            let (name, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            tags.push(Tag {
                name: name.to_string(),
                value: value.trim().to_string(),
            });
        }
    }
    tags
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        &self.toks[self.pos]
    }

    fn peek_at(&self, n: usize) -> &Token {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)]
    }

    fn at_eof(&self) -> bool {
        self.peek().tok == Tok::Eof
    }

    fn next(&mut self) -> &Token {
        let t = &self.toks[self.pos];
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn is(&self, p: &str) -> bool {
        matches!(&self.peek().tok, Tok::Punct(q) if *q == p)
    }

    fn is_ident(&self, name: &str) -> bool {
        matches!(&self.peek().tok, Tok::Ident(n) if n == name)
    }

    fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.next();
            true
        } else {
            false
        }
    }

    fn err<T>(&self, span: Span, msg: impl Into<String>) -> PResult<T> {
        Err(Diagnostic::new(span, msg))
    }

    fn describe(t: &Tok) -> String {
        match t {
            Tok::Ident(n) => format!("`{n}`"),
            Tok::Num(_, s) => format!("`{s}`"),
            Tok::Str(_) => "a string".into(),
            Tok::Punct(p) => format!("`{p}`"),
            Tok::Eof => "the end of the file".into(),
        }
    }

    fn expect(&mut self, p: &str) -> PResult<Span> {
        if self.is(p) {
            Ok(self.next().span)
        } else {
            let t = self.peek();
            self.err(
                t.span,
                format!("expected `{p}`, found {}", Self::describe(&t.tok)),
            )
        }
    }

    fn expect_ident(&mut self, name: &str) -> PResult<Span> {
        if self.is_ident(name) {
            Ok(self.next().span)
        } else {
            let t = self.peek();
            self.err(
                t.span,
                format!("expected `{name}`, found {}", Self::describe(&t.tok)),
            )
        }
    }

    /// A binding or callee name (not a keyword).
    fn ident(&mut self) -> PResult<(String, Span)> {
        let t = self.peek().clone();
        match &t.tok {
            Tok::Ident(n) if n == "arguments" || n == "eval" => {
                self.err(t.span, format!("`{n}` is not in the subset"))
            }
            Tok::Ident(n) if KEYWORDS.contains(&n.as_str()) => {
                self.err(t.span, format!("expected a name, found the keyword `{n}`"))
            }
            Tok::Ident(n) => {
                self.next();
                Ok((n.clone(), t.span))
            }
            other => self.err(
                t.span,
                format!("expected a name, found {}", Self::describe(other)),
            ),
        }
    }

    fn skip_to_semicolon(&mut self) -> PResult<()> {
        let mut depth = 0i32;
        loop {
            let t = self.next().clone();
            match t.tok {
                Tok::Eof => return self.err(t.span, "expected `;`"),
                Tok::Punct("{" | "(" | "[") => depth += 1,
                Tok::Punct("}" | ")" | "]") => depth -= 1,
                Tok::Punct(";") if depth == 0 => return Ok(()),
                _ => {}
            }
        }
    }

    fn type_ref(&mut self) -> PResult<TypeRef> {
        self.type_ref_in(false)
    }

    /// A type name; `array_ok` allows a following `[]` (the caller parses it).
    fn type_ref_in(&mut self, array_ok: bool) -> PResult<TypeRef> {
        let t = self.peek().clone();
        if let Tok::Ident(n) = &t.tok
            && let Some(ty) = Type::from_name(n)
        {
            self.next();
            if self.is("|") || (self.is("[") && !array_ok) || self.is("<") || self.is("&") {
                return self.err(
                    self.peek().span,
                    "only the plain type names of intrinsics.d.ts are in the subset",
                );
            }
            return Ok(TypeRef { ty, span: t.span });
        }
        self.err(
            t.span,
            format!(
                "expected a type (Value, Obj, Fn, Str, Sym, boolean, i32, u32, f64, void, never), found {}",
                Self::describe(&t.tok)
            ),
        )
    }

    /// `(params)`; returns the `this` type and the parameters.
    fn params(&mut self) -> PResult<(Option<TypeRef>, Vec<Param>)> {
        self.expect("(")?;
        let mut this_ty = None;
        let mut params: Vec<Param> = Vec::new();
        while !self.is(")") {
            let start = self.peek().span;
            if self.is_ident("this") && params.is_empty() && this_ty.is_none() {
                self.next();
                self.expect(":")?;
                this_ty = Some(self.type_ref()?);
            } else {
                // Builtins may take `...args: Value[]` too: the local is an array (Obj).
                let rest = self.eat("...");
                let (name, _) = self.ident()?;
                let optional = self.eat("?");
                if self.is("=") {
                    return self.err(
                        self.peek().span,
                        "default parameter values are not in the subset",
                    );
                }
                self.expect(":")?;
                let ty = self.type_ref_in(rest)?;
                if self.is("=") {
                    return self.err(
                        self.peek().span,
                        "default parameter values are not in the subset",
                    );
                }
                let mut end = ty.span;
                if rest {
                    self.expect("[")?;
                    end = self.expect("]")?;
                }
                if !optional && !rest && params.iter().any(|p| p.optional) {
                    return self.err(start, "a required parameter cannot follow an optional one");
                }
                if params.iter().any(|p| p.rest) {
                    return self.err(start, "the rest parameter must be last");
                }
                params.push(Param {
                    name,
                    ty,
                    optional,
                    rest,
                    span: start.to(end),
                });
            }
            if !self.eat(",") {
                break;
            }
        }
        self.expect(")")?;
        Ok((this_ty, params))
    }

    fn function(&mut self) -> PResult<Function> {
        let t = self.peek().clone();
        let doc = t.doc.clone();
        match &t.tok {
            Tok::Ident(k) if k == "function" => {}
            Tok::Ident(k) if k == "async" => {
                return self.err(t.span, "async functions are not in the subset");
            }
            Tok::Ident(k) if k == "export" || k == "import" => {
                return self.err(
                    t.span,
                    "builtin files are scripts: no import/export (intrinsics.d.ts is global)",
                );
            }
            Tok::Ident(k)
                if matches!(
                    k.as_str(),
                    "const" | "let" | "var" | "class" | "type" | "interface"
                ) =>
            {
                return self.err(
                    t.span,
                    "only function declarations are allowed at the top level",
                );
            }
            other => {
                return self.err(
                    t.span,
                    format!(
                        "expected a function declaration, found {}",
                        Self::describe(other)
                    ),
                );
            }
        }
        self.next();
        if self.is("*") {
            return self.err(
                self.peek().span,
                "generators are not in the subset; write an explicit state object",
            );
        }
        let (name, name_span) = self.ident()?;
        if self.is("<") {
            return self.err(self.peek().span, "type parameters are not in the subset");
        }
        let (this_ty, params) = self.params()?;
        if !self.is(":") {
            return self.err(
                self.peek().span,
                "the return type must be written explicitly",
            );
        }
        self.next();
        let ret = self.type_ref()?;
        let body = self.block()?;
        let span = t.span.to(body.span);
        Ok(Function {
            name,
            this_ty,
            params,
            ret,
            body,
            tags: parse_tags(doc.as_deref()),
            span,
            name_span,
        })
    }

    fn block(&mut self) -> PResult<Block> {
        let start = self.expect("{")?;
        let mut stmts = Vec::new();
        while !self.is("}") {
            if self.at_eof() {
                return self.err(self.peek().span, "expected `}`");
            }
            stmts.push(self.stmt()?);
        }
        let end = self.expect("}")?;
        Ok(Block {
            stmts,
            span: start.to(end),
        })
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let t = self.peek().clone();
        if self.is("{") {
            return Ok(Stmt::Block(self.block()?));
        }
        if self.is(";") {
            return self.err(t.span, "empty statements are not in the subset");
        }
        let Tok::Ident(word) = &t.tok else {
            let s = self.simple_stmt()?;
            self.expect(";")?;
            return Ok(s);
        };
        match word.as_str() {
            "const" | "let" => {
                let d = self.decl()?;
                self.expect(";")?;
                Ok(d)
            }
            "var" => self.err(t.span, "use `let` or `const` instead of `var`"),
            "if" => {
                self.next();
                self.expect("(")?;
                let cond = self.expr()?;
                self.expect(")")?;
                let then = Box::new(self.stmt()?);
                let els = if self.is_ident("else") {
                    self.next();
                    Some(Box::new(self.stmt()?))
                } else {
                    None
                };
                let end = els
                    .as_ref()
                    .map(|s| stmt_span(s))
                    .unwrap_or_else(|| stmt_span(&then));
                Ok(Stmt::If {
                    cond,
                    then,
                    els,
                    span: t.span.to(end),
                })
            }
            "while" => {
                self.next();
                self.expect("(")?;
                let cond = self.expr()?;
                self.expect(")")?;
                let body = Box::new(self.stmt()?);
                let span = t.span.to(stmt_span(&body));
                Ok(Stmt::While { cond, body, span })
            }
            "do" => {
                self.next();
                let body = Box::new(self.stmt()?);
                self.expect_ident("while")?;
                self.expect("(")?;
                let cond = self.expr()?;
                self.expect(")")?;
                let end = self.expect(";")?;
                Ok(Stmt::DoWhile {
                    body,
                    cond,
                    span: t.span.to(end),
                })
            }
            "for" => {
                self.next();
                self.expect("(")?;
                if matches!(&self.peek_at(2).tok, Tok::Ident(w) if w == "of" || w == "in") {
                    return self.err(
                        self.peek_at(2).span,
                        "for-in and for-of are not in the subset; use an index loop",
                    );
                }
                let init = if self.is(";") {
                    None
                } else if self.is_ident("const") || self.is_ident("let") {
                    Some(Box::new(self.decl()?))
                } else {
                    Some(Box::new(self.simple_stmt()?))
                };
                if self.is_ident("of") || self.is_ident("in") {
                    return self.err(
                        self.peek().span,
                        "for-in and for-of are not in the subset; use an index loop",
                    );
                }
                self.expect(";")?;
                let cond = if self.is(";") {
                    None
                } else {
                    Some(self.expr()?)
                };
                self.expect(";")?;
                let update = if self.is(")") {
                    None
                } else {
                    Some(Box::new(self.simple_stmt()?))
                };
                self.expect(")")?;
                let body = Box::new(self.stmt()?);
                let span = t.span.to(stmt_span(&body));
                Ok(Stmt::For {
                    init,
                    cond,
                    update,
                    body,
                    span,
                })
            }
            "return" => {
                self.next();
                let value = if self.is(";") {
                    None
                } else {
                    Some(self.expr()?)
                };
                let end = self.expect(";")?;
                Ok(Stmt::Return {
                    value,
                    span: t.span.to(end),
                })
            }
            "throw" => {
                self.next();
                let value = self.expr()?;
                let end = self.expect(";")?;
                Ok(Stmt::Throw {
                    value,
                    span: t.span.to(end),
                })
            }
            "try" => {
                self.next();
                let body = self.block()?;
                let catch = if self.is_ident("catch") {
                    self.next();
                    let name = if self.eat("(") {
                        let n = self.ident()?;
                        if self.eat(":") {
                            let ty = self.type_ref()?;
                            if ty.ty != Type::Value {
                                return self.err(ty.span, "a catch binding has type Value");
                            }
                        }
                        self.expect(")")?;
                        Some(n)
                    } else {
                        None
                    };
                    Some(Catch {
                        name,
                        body: self.block()?,
                    })
                } else {
                    None
                };
                let finally = if self.is_ident("finally") {
                    self.next();
                    Some(self.block()?)
                } else {
                    None
                };
                if catch.is_none() && finally.is_none() {
                    return self.err(t.span, "`try` needs `catch` or `finally`");
                }
                let end = finally
                    .as_ref()
                    .map(|b| b.span)
                    .or(catch.as_ref().map(|c| c.body.span))
                    .unwrap();
                Ok(Stmt::Try {
                    body,
                    catch,
                    finally,
                    span: t.span.to(end),
                })
            }
            "break" | "continue" => {
                self.next();
                if matches!(self.peek().tok, Tok::Ident(_)) {
                    return self.err(self.peek().span, "labels are not in the subset");
                }
                let end = self.expect(";")?;
                let span = t.span.to(end);
                Ok(if word == "break" {
                    Stmt::Break(span)
                } else {
                    Stmt::Continue(span)
                })
            }
            "switch" => self.err(t.span, "`switch` is not in the subset; use if/else"),
            "with" => self.err(t.span, "`with` is not in the subset"),
            "function" | "class" => {
                self.err(t.span, "nested functions and classes are not in the subset")
            }
            _ if matches!(self.peek_at(1).tok, Tok::Punct(":")) => {
                self.err(t.span, "labels are not in the subset")
            }
            _ => {
                let s = self.simple_stmt()?;
                self.expect(";")?;
                Ok(s)
            }
        }
    }

    /// `const|let name: T = expr` (without the semicolon).
    fn decl(&mut self) -> PResult<Stmt> {
        let t = self.next().clone();
        let kind = if t.tok == Tok::Ident("const".into()) {
            DeclKind::Const
        } else {
            DeclKind::Let
        };
        let (name, name_span) = self.ident()?;
        if !self.is(":") {
            return self.err(name_span, format!("`{name}` needs a type annotation"));
        }
        self.next();
        let ty = self.type_ref()?;
        if !self.is("=") {
            return self.err(self.peek().span, format!("`{name}` needs an initializer"));
        }
        self.next();
        let init = self.expr()?;
        let span = t.span.to(init.span);
        Ok(Stmt::Decl {
            kind,
            name,
            ty,
            init,
            span,
        })
    }

    /// An assignment, an update (`x++`) or an expression, without the semicolon.
    fn simple_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        if self.is("++") || self.is("--") {
            let inc = self.next().tok == Tok::Punct("++");
            let (target, span) = self.ident()?;
            return Ok(Stmt::Update {
                target,
                inc,
                span: start.to(span),
            });
        }
        if let Tok::Ident(_) = &self.peek().tok
            && let Tok::Punct(p) = self.peek_at(1).tok
        {
            if p == "++" || p == "--" {
                let (target, _) = self.ident()?;
                let end = self.next().span;
                return Ok(Stmt::Update {
                    target,
                    inc: p == "++",
                    span: start.to(end),
                });
            }
            if let Some(op) = assign_op(p) {
                let (target, target_span) = self.ident()?;
                self.next();
                let value = self.expr()?;
                let span = start.to(value.span);
                return Ok(Stmt::Assign {
                    target,
                    target_span,
                    op,
                    value,
                    span,
                });
            }
            if matches!(p, "**=" | ">>>=" | "&&=" | "||=" | "??=") {
                return self.err(self.peek_at(1).span, format!("`{p}` is not in the subset"));
            }
        }
        let e = self.expr()?;
        if let Tok::Punct(p) = self.peek().tok
            && (assign_op(p).is_some() || p == "++" || p == "--")
        {
            return self.err(self.peek().span, "only local variables can be assigned");
        }
        Ok(Stmt::Expr(e))
    }

    fn expr(&mut self) -> PResult<Expr> {
        let cond = self.binary(1)?;
        if self.eat("?") {
            let a = self.expr()?;
            self.expect(":")?;
            let b = self.expr()?;
            let span = cond.span.to(b.span);
            return Ok(Expr {
                kind: ExprKind::Cond(Box::new(cond), Box::new(a), Box::new(b)),
                span,
            });
        }
        if let Tok::Punct(p) = self.peek().tok {
            if assign_op(p).is_some() {
                return self.err(
                    self.peek().span,
                    "assignments are statements in the subset, not expressions",
                );
            }
            if p == "=>" {
                return self.err(self.peek().span, "arrow functions are not in the subset");
            }
        }
        Ok(cond)
    }

    fn binary(&mut self, min_prec: u8) -> PResult<Expr> {
        let mut left = self.unary()?;
        loop {
            let t = self.peek().clone();
            let op = match &t.tok {
                Tok::Punct(p) => match bin_op(p) {
                    Some(op) => op,
                    None => {
                        if let Some(msg) = forbidden_binary(p) {
                            return self.err(t.span, msg);
                        }
                        break;
                    }
                },
                Tok::Ident(w)
                    if w == "in" || w == "instanceof" || w == "as" || w == "satisfies" =>
                {
                    return self.err(t.span, format!("`{w}` is not in the subset"));
                }
                _ => break,
            };
            let prec = op.precedence();
            if prec < min_prec {
                break;
            }
            self.next();
            let right = self.binary(prec + 1)?;
            let span = left.span.to(right.span);
            left = Expr {
                kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
                span,
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let t = self.peek().clone();
        let op = match &t.tok {
            Tok::Punct("!") => Some(UnOp::Not),
            Tok::Punct("-") => Some(UnOp::Neg),
            Tok::Punct("~") => Some(UnOp::BitNot),
            Tok::Punct("+") => {
                return self.err(
                    t.span,
                    "unary `+` is not in the subset; use a conversion intrinsic",
                );
            }
            Tok::Punct("++" | "--") => {
                return self.err(t.span, "`++` and `--` are statements in the subset");
            }
            Tok::Ident(w)
                if matches!(w.as_str(), "typeof" | "void" | "delete" | "await" | "yield") =>
            {
                return self.err(t.span, format!("`{w}` is not in the subset"));
            }
            _ => None,
        };
        if let Some(op) = op {
            self.next();
            let e = self.unary()?;
            let span = t.span.to(e.span);
            return Ok(Expr {
                kind: ExprKind::Unary(op, Box::new(e)),
                span,
            });
        }
        let e = self.primary()?;
        match &self.peek().tok {
            Tok::Punct("." | "[" | "?.") => self.err(
                self.peek().span,
                "property access is not in the subset; use an intrinsic such as Get(o, key)",
            ),
            Tok::Punct("(") => self.err(self.peek().span, "only named functions can be called"),
            Tok::Punct("++" | "--") => self.err(
                self.peek().span,
                "`++` and `--` are statements in the subset",
            ),
            Tok::Punct("!") => self.err(
                self.peek().span,
                "non-null assertions are not in the subset",
            ),
            _ => Ok(e),
        }
    }

    fn primary(&mut self) -> PResult<Expr> {
        let t = self.peek().clone();
        let kind = match &t.tok {
            Tok::Num(v, s) => {
                self.next();
                ExprKind::Num(*v, s.clone())
            }
            Tok::Str(v) => {
                self.next();
                ExprKind::Str(v.clone())
            }
            Tok::Punct("(") => {
                self.next();
                if self.is(")")
                    || matches!(
                        (&self.peek().tok, &self.peek_at(1).tok),
                        (Tok::Ident(_), Tok::Punct(":" | ","))
                    )
                {
                    return self.err(t.span, "arrow functions are not in the subset");
                }
                let e = self.expr()?;
                let end = self.expect(")")?;
                return Ok(Expr {
                    kind: ExprKind::Paren(Box::new(e)),
                    span: t.span.to(end),
                });
            }
            Tok::Punct("{" | "[") => {
                return self.err(
                    t.span,
                    "object and array literals are not in the subset; use intrinsics",
                );
            }
            Tok::Ident(w) => match w.as_str() {
                "true" | "false" => {
                    self.next();
                    ExprKind::Bool(w == "true")
                }
                "null" => {
                    self.next();
                    ExprKind::Null
                }
                "undefined" => {
                    self.next();
                    ExprKind::Undefined
                }
                "this" => {
                    self.next();
                    ExprKind::This
                }
                "new"
                    if matches!(self.peek_at(1).tok, Tok::Punct("."))
                        && matches!(&self.peek_at(2).tok, Tok::Ident(n) if n == "target") =>
                {
                    self.next();
                    self.next();
                    let end = self.next().span;
                    return Ok(Expr {
                        kind: ExprKind::NewTarget,
                        span: t.span.to(end),
                    });
                }
                "new" => return self.err(t.span, "`new` is not in the subset; use Construct(...)"),
                "function" | "class" | "async" => {
                    return self.err(
                        t.span,
                        "function expressions and closures are not in the subset",
                    );
                }
                "super" => return self.err(t.span, "`super` is not in the subset"),
                _ => {
                    let (name, span) = self.ident()?;
                    if self.is("(") {
                        self.next();
                        let mut args = Vec::new();
                        while !self.is(")") {
                            if self.is("...") {
                                return self.err(
                                    self.peek().span,
                                    "spread arguments are not in the subset",
                                );
                            }
                            args.push(self.expr()?);
                            if !self.eat(",") {
                                break;
                            }
                        }
                        let end = self.expect(")")?;
                        return Ok(Expr {
                            kind: ExprKind::Call {
                                callee: name,
                                callee_span: span,
                                args,
                            },
                            span: span.to(end),
                        });
                    }
                    if self.is("<")
                        && matches!(self.peek_at(1).tok, Tok::Ident(_))
                        && matches!(self.peek_at(2).tok, Tok::Punct(">"))
                    {
                        return self.err(self.peek().span, "type arguments are not in the subset");
                    }
                    ExprKind::Ident(name)
                }
            },
            other => {
                return self.err(
                    t.span,
                    format!("expected an expression, found {}", Self::describe(other)),
                );
            }
        };
        Ok(Expr { kind, span: t.span })
    }
}

fn assign_op(p: &str) -> Option<AssignOp> {
    Some(match p {
        "=" => AssignOp::Assign,
        "+=" => AssignOp::Add,
        "-=" => AssignOp::Sub,
        "*=" => AssignOp::Mul,
        "/=" => AssignOp::Div,
        "%=" => AssignOp::Rem,
        "&=" => AssignOp::BitAnd,
        "|=" => AssignOp::BitOr,
        "^=" => AssignOp::BitXor,
        "<<=" => AssignOp::Shl,
        ">>=" => AssignOp::Shr,
        _ => return None,
    })
}

fn bin_op(p: &str) -> Option<BinOp> {
    Some(match p {
        "+" => BinOp::Add,
        "-" => BinOp::Sub,
        "*" => BinOp::Mul,
        "/" => BinOp::Div,
        "%" => BinOp::Rem,
        "&" => BinOp::BitAnd,
        "|" => BinOp::BitOr,
        "^" => BinOp::BitXor,
        "<<" => BinOp::Shl,
        ">>" => BinOp::Shr,
        ">>>" => BinOp::UShr,
        "<" => BinOp::Lt,
        "<=" => BinOp::Le,
        ">" => BinOp::Gt,
        ">=" => BinOp::Ge,
        "===" => BinOp::StrictEq,
        "!==" => BinOp::StrictNe,
        "&&" => BinOp::And,
        "||" => BinOp::Or,
        _ => return None,
    })
}

fn forbidden_binary(p: &str) -> Option<&'static str> {
    Some(match p {
        "==" | "!=" => "`==` and `!=` may run user code (ToPrimitive); use `===` / `!==`",
        "**" => "`**` is not in the subset; use an intrinsic",
        "??" => "`??` is not in the subset; compare with `undefined`/`null` explicitly",
        _ => return None,
    })
}

pub fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Decl { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::Update { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. }
        | Stmt::DoWhile { span, .. }
        | Stmt::For { span, .. }
        | Stmt::Return { span, .. }
        | Stmt::Throw { span, .. }
        | Stmt::Try { span, .. } => *span,
        Stmt::Expr(e) => e.span,
        Stmt::Break(s) | Stmt::Continue(s) => *s,
        Stmt::Block(b) => b.span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(src: &str) -> Module {
        parse_module(0, src).unwrap_or_else(|e| panic!("{e:?}"))
    }

    fn error(src: &str) -> String {
        parse_module(0, src).unwrap_err().message
    }

    #[test]
    fn function_and_statements() {
        let m = module(
            "/** Docs.\n * @builtin Array.prototype.map\n * @length 1\n */\n\
             function F(this: Value, a: Value, b?: Value): Obj {\n\
               const o: Obj = ToObject(this);\n\
               let k: f64 = 0;\n\
               for (let i: u32 = 0; i < 10; i++) { k += 1; }\n\
               while (k > 0) { k = k - 1; if (k === 3) { break; } else continue; }\n\
               do { k++; } while (false);\n\
               try { Call(a, b); } catch (e) { throw e; } finally { --k; }\n\
               return o;\n\
             }",
        );
        let f = &m.functions[0];
        assert_eq!(f.name, "F");
        assert_eq!(f.this_ty.as_ref().unwrap().ty, Type::Value);
        assert_eq!(f.params.len(), 2);
        assert!(f.params[1].optional);
        assert_eq!(f.ret.ty, Type::Obj);
        assert_eq!(
            f.tags,
            [
                Tag {
                    name: "builtin".into(),
                    value: "Array.prototype.map".into()
                },
                Tag {
                    name: "length".into(),
                    value: "1".into()
                }
            ]
        );
        assert_eq!(f.body.stmts.len(), 7);
    }

    #[test]
    fn precedence() {
        let m = module("function f(): void { g(1 + 2 * 3 === 7 && !h(), a < b ? c : d - -e); }");
        let Stmt::Expr(Expr {
            kind: ExprKind::Call { args, .. },
            ..
        }) = &m.functions[0].body.stmts[0]
        else {
            panic!()
        };
        let ExprKind::Binary(BinOp::And, l, _) = &args[0].kind else {
            panic!("{:?}", args[0])
        };
        let ExprKind::Binary(BinOp::StrictEq, l, _) = &l.kind else {
            panic!()
        };
        assert!(
            matches!(&l.kind, ExprKind::Binary(BinOp::Add, _, r) if matches!(r.kind, ExprKind::Binary(BinOp::Mul, ..)))
        );
        assert!(matches!(args[1].kind, ExprKind::Cond(..)));
    }

    #[test]
    fn rejections() {
        let cases = [
            ("function f(): void { o.x; }", "property access"),
            ("function f(): void { o[0]; }", "property access"),
            ("function f(): void { new X(); }", "Construct"),
            ("function f(): void { const g: Value = () => 1; }", "arrow"),
            (
                "function f(): void { function g(): void {} }",
                "nested functions",
            ),
            ("function* f(): void {}", "generators"),
            ("async function f(): void {}", "async"),
            ("function f(): void { var x: i32 = 1; }", "let"),
            ("function f(): void { let x = 1; }", "type annotation"),
            ("function f(): void { let x: i32; }", "initializer"),
            ("function f(): void { g(arguments); }", "`arguments`"),
            ("function f(): void { eval(1); }", "`eval`"),
            ("function f(): void { if (a == b) {} }", "==="),
            ("function f(): void { g(typeof a); }", "typeof"),
            ("function f(): void { g(x = 1); }", "statements"),
            ("function f(): void { g(x++); }", "statements"),
            ("function f(): void { g({}); }", "literals"),
            ("function f(): void { switch (x) {} }", "switch"),
            ("function f(): void { for (const x of y) {} }", "for-of"),
            ("function f(): void { l: while (true) {} }", "labels"),
            ("function f() { }", "return type"),
            ("function f(a: number): void {}", "expected a type"),
            ("function f(a: Value[]): void {}", "plain type names"),
            ("function f(...a: Value): void {}", "expected `[`"),
            (
                "function f(a?: Value, b: Value): void {}",
                "required parameter",
            ),
            ("function f(a: Value = 1): void {}", "default"),
            ("function f(): void { g(a as Obj); }", "`as`"),
            ("function f(): void { g(...a); }", "spread"),
            ("export function f(): void {}", "import/export"),
            ("const x: i32 = 1;", "top level"),
            ("function f(): void { g(a ?? b); }", "??"),
            ("function f(): void { g(+a); }", "unary `+`"),
            ("function f(): void { ; }", "empty statements"),
            ("function f(): void { f()(); }", "named functions"),
            ("function f(): void { try {} }", "catch"),
            (
                "function f(): void { try {} catch (e: Obj) {} }",
                "type Value",
            ),
        ];
        for (src, want) in cases {
            let e = error(src);
            assert!(e.contains(want), "{src}: `{e}` does not mention `{want}`");
        }
    }

    #[test]
    fn tags() {
        let t = parse_tags(Some(
            " Does X. @leaf\n * @builtin A.b @length 2\n * mail me@example.com\n",
        ));
        let names: Vec<(&str, &str)> = t
            .iter()
            .map(|t| (t.name.as_str(), t.value.as_str()))
            .collect();
        assert_eq!(names, [("leaf", ""), ("builtin", "A.b"), ("length", "2")]);
    }

    #[test]
    fn declarations() {
        let d = parse_declarations(
            0,
            "type Value = unknown;\ntype i32 = number & { readonly __num?: \"i32\" };\n\
             /** Calls f.\n * @reentrant\n */\ndeclare function Call(f: Value, thisArg: Value, ...args: Value[]): Value;\n\
             /** @leaf */ declare function IsObject(v: Value): boolean;\n",
        )
        .unwrap();
        assert_eq!(d.intrinsics.len(), 2);
        assert_eq!(d.intrinsics[0].name, "Call");
        assert!(d.intrinsics[0].params[2].rest);
        assert_eq!(d.intrinsics[0].tags[0].name, "reentrant");
        assert_eq!(d.intrinsics[1].ret.ty, Type::Bool);
        assert!(
            parse_declarations(0, "declare function F(...a: Value[], b: Value): void;").is_err()
        );
        assert!(parse_declarations(0, "function F(): void;").is_err());
    }
}
