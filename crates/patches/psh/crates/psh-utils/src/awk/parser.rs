// Ported from GoAWK's parser (parser/parser.go),
// Copyright (c) 2022 Ben Hoyt, MIT License (see NOTICE).

//! The awk parser: recursive descent over the lexer's tokens, into [`ast::Program`].

use super::ast::*;
use super::lexer::{Builtin, Lexer, Pos, Tok};

#[derive(Debug, Clone)]
pub struct ParseError {
    pub pos: Pos,
    pub msg: String,
}

type PResult<T> = Result<T, ParseError>;

pub fn parse(src: &[u8]) -> PResult<Program> {
    let mut p = Parser {
        lexer: Lexer::new(src),
        pos: Pos::default(),
        tok: Tok::Illegal,
        prev_tok: Tok::Illegal,
        val: Vec::new(),
        in_action: false,
        in_func: false,
        loop_depth: 0,
        pending_getline_left: None,
        multis: Vec::new(),
    };
    p.next()?;
    p.program()
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    pos: Pos,
    tok: Tok,
    prev_tok: Tok,
    val: Vec<u8>,
    in_action: bool,
    in_func: bool,
    loop_depth: usize,
    /// The expression left of `|` in `cmd | getline`, parsed before the `|` showed what
    /// it was.
    pending_getline_left: Option<Expr>,
    /// Comma-separated `(a, b)` expressions not (yet) used by print or `in`.
    multis: Vec<Pos>,
}

/// Expressions of print and printf arguments do not take `>` (a redirection) and `|`
/// (a pipe) as operators unless in parentheses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Full,
    Print,
}

impl Parser<'_> {
    fn program(&mut self) -> PResult<Program> {
        let mut prog = Program::default();
        // Items are separated by `;` or newlines, which may be left out after one that
        // ends with a brace and after the last.
        let mut needs_terminator = false;
        while self.tok != Tok::Eof {
            if needs_terminator {
                if !self.matches(&[Tok::Newline, Tok::Semicolon]) {
                    return Err(self.error("syntax error"));
                }
                self.next()?;
                needs_terminator = false;
            }
            self.optional_newlines()?;
            match self.tok {
                Tok::Eof => {}
                Tok::Begin => {
                    self.next()?;
                    let body = self.stmts_brace()?;
                    prog.begin.push(body);
                }
                Tok::End => {
                    self.next()?;
                    let body = self.stmts_brace()?;
                    prog.end.push(body);
                }
                Tok::Function => {
                    let f = self.function()?;
                    prog.functions.push(f);
                }
                Tok::Semicolon => {
                    self.next()?;
                }
                _ => {
                    self.in_action = true;
                    let mut pattern = Vec::new();
                    if !self.matches(&[Tok::LBrace, Tok::Eof]) {
                        pattern.push(self.expr()?);
                    }
                    if !self.matches(&[Tok::LBrace, Tok::Eof, Tok::Newline, Tok::Semicolon]) {
                        self.comma_newlines()?;
                        pattern.push(self.expr()?);
                    }
                    let body = if self.tok == Tok::LBrace {
                        Some(self.stmts_brace()?)
                    } else {
                        needs_terminator = true;
                        None
                    };
                    prog.actions.push(Action { pattern, body });
                    self.in_action = false;
                }
            }
        }
        if let Some(pos) = self.multis.iter().min_by_key(|p| (p.line, p.col)) {
            return Err(ParseError {
                pos: *pos,
                msg: "syntax error".into(),
            });
        }
        Ok(prog)
    }

    /// A statement list: `{ ... }`, a single statement, or `;` alone (an empty body).
    fn stmts(&mut self) -> PResult<Vec<Stmt>> {
        match self.tok {
            Tok::Semicolon => {
                self.next()?;
                Ok(Vec::new())
            }
            Tok::LBrace => self.stmts_brace(),
            _ => Ok(vec![self.stmt()?]),
        }
    }

    fn stmts_brace(&mut self) -> PResult<Vec<Stmt>> {
        self.expect(Tok::LBrace)?;
        self.optional_newlines()?;
        let mut ss = Vec::new();
        let mut line = 0;
        while self.tok != Tok::RBrace && self.tok != Tok::Eof {
            if self.matches(&[Tok::Semicolon, Tok::Newline]) {
                self.next()?;
                continue;
            }
            // Where statements start, for the line of a run-time error.
            if self.pos.line != line {
                line = self.pos.line;
                ss.push(Stmt::Line(line));
            }
            ss.push(self.stmt()?);
        }
        self.expect(Tok::RBrace)?;
        if self.tok == Tok::Semicolon {
            self.next()?;
        }
        Ok(ss)
    }

    /// A simple statement (one allowed in a `for` clause).
    fn simple_stmt(&mut self) -> PResult<Stmt> {
        match self.tok {
            Tok::Print | Tok::Printf => {
                let printf = self.tok == Tok::Printf;
                self.next()?;
                let mut args = self.expr_list(Ctx::Print)?;
                if args.len() == 1 && matches!(args[0], Expr::Multi(..)) {
                    // Parentheses around all the arguments.
                    if let Some(Expr::Multi(exprs, pos)) = args.pop() {
                        self.multis.retain(|p| *p != pos);
                        args = exprs;
                    }
                }
                let (redirect, dest) = match self.tok {
                    Tok::Greater | Tok::Append | Tok::Pipe => {
                        let r = match self.tok {
                            Tok::Greater => Redirect::Write,
                            Tok::Append => Redirect::Append,
                            _ => Redirect::Pipe,
                        };
                        self.next()?;
                        // The destination is a concatenation at most: `print > "a" x`.
                        (r, Some(self.concat(Ctx::Print)?))
                    }
                    _ => (Redirect::None, None),
                };
                if printf {
                    if args.is_empty() {
                        return Err(self.error("fatal: printf: no arguments"));
                    }
                    Ok(Stmt::Printf {
                        args,
                        redirect,
                        dest,
                    })
                } else {
                    Ok(Stmt::Print {
                        args,
                        redirect,
                        dest,
                    })
                }
            }
            Tok::Delete => {
                self.next()?;
                let (array, pos) = self.expect_name()?;
                let mut index = Vec::new();
                if self.tok == Tok::LBracket {
                    self.next()?;
                    index = self.expr_list(Ctx::Full)?;
                    if index.is_empty() {
                        return Err(self.error("syntax error"));
                    }
                    self.expect(Tok::RBracket)?;
                }
                Ok(Stmt::Delete { array, pos, index })
            }
            Tok::If
            | Tok::For
            | Tok::While
            | Tok::Do
            | Tok::Break
            | Tok::Continue
            | Tok::Next
            | Tok::Nextfile
            | Tok::Exit
            | Tok::Return => Err(self.error("syntax error")),
            _ => Ok(Stmt::Expr(self.expr()?)),
        }
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let s = match self.tok {
            Tok::If => {
                self.next()?;
                self.expect(Tok::LParen)?;
                let cond = self.expr()?;
                self.expect(Tok::RParen)?;
                self.optional_newlines()?;
                let body = self.stmts()?;
                self.optional_newlines()?;
                let mut else_ = Vec::new();
                if self.tok == Tok::Else {
                    self.next()?;
                    self.optional_newlines()?;
                    else_ = self.stmts()?;
                }
                Stmt::If { cond, body, else_ }
            }
            Tok::For => {
                self.next()?;
                self.expect(Tok::LParen)?;
                let mut pre = None;
                if self.tok != Tok::Semicolon {
                    pre = Some(self.simple_stmt()?);
                }
                if pre.is_some() && self.tok == Tok::RParen {
                    // for (var in array) body
                    self.next()?;
                    self.optional_newlines()?;
                    let (var, var_pos, array, array_pos) = match pre {
                        Some(Stmt::Expr(Expr::In(mut index, array, array_pos)))
                            if index.len() == 1 && matches!(index[0], Expr::Var(..)) =>
                        {
                            let Some(Expr::Var(var, var_pos)) = index.pop() else {
                                unreachable!()
                            };
                            (var, var_pos, array, array_pos)
                        }
                        _ => return Err(self.error("syntax error")),
                    };
                    let body = self.loop_stmts()?;
                    Stmt::ForIn {
                        var,
                        var_pos,
                        array,
                        array_pos,
                        body,
                    }
                } else {
                    self.expect(Tok::Semicolon)?;
                    self.optional_newlines()?;
                    let mut cond = None;
                    if self.tok != Tok::Semicolon {
                        cond = Some(self.expr()?);
                    }
                    self.expect(Tok::Semicolon)?;
                    self.optional_newlines()?;
                    let mut post = None;
                    if self.tok != Tok::RParen {
                        post = Some(Box::new(self.simple_stmt()?));
                    }
                    self.expect(Tok::RParen)?;
                    self.optional_newlines()?;
                    let body = self.loop_stmts()?;
                    Stmt::For {
                        pre: pre.map(Box::new),
                        cond,
                        post,
                        body,
                    }
                }
            }
            Tok::While => {
                self.next()?;
                self.expect(Tok::LParen)?;
                let cond = self.expr()?;
                self.expect(Tok::RParen)?;
                if self.tok == Tok::Semicolon {
                    // `while (x);`: an empty body.
                    self.next()?;
                    Stmt::While {
                        cond,
                        body: Vec::new(),
                    }
                } else {
                    self.optional_newlines()?;
                    let body = self.loop_stmts()?;
                    Stmt::While { cond, body }
                }
            }
            Tok::Do => {
                self.next()?;
                self.optional_newlines()?;
                let body = self.loop_stmts()?;
                self.optional_newlines()?;
                self.expect(Tok::While)?;
                self.expect(Tok::LParen)?;
                let cond = self.expr()?;
                self.expect(Tok::RParen)?;
                Stmt::DoWhile { body, cond }
            }
            Tok::Break | Tok::Continue => {
                if self.loop_depth == 0 {
                    let what = if self.tok == Tok::Break {
                        "break"
                    } else {
                        "continue"
                    };
                    return Err(self.error(&format!("`{what}' is not allowed outside a loop")));
                }
                let s = if self.tok == Tok::Break {
                    Stmt::Break
                } else {
                    Stmt::Continue
                };
                self.next()?;
                s
            }
            Tok::Next | Tok::Nextfile => {
                if !self.in_action && !self.in_func {
                    let what = if self.tok == Tok::Next {
                        "next"
                    } else {
                        "nextfile"
                    };
                    return Err(self.error(&format!("`{what}' used in BEGIN or END action")));
                }
                let s = if self.tok == Tok::Next {
                    Stmt::Next
                } else {
                    Stmt::Nextfile
                };
                self.next()?;
                s
            }
            Tok::Exit => {
                self.next()?;
                let mut status = None;
                if !self.matches(&[Tok::Newline, Tok::Semicolon, Tok::RBrace, Tok::Eof]) {
                    status = Some(self.expr()?);
                }
                Stmt::Exit(status)
            }
            Tok::Return => {
                if !self.in_func {
                    return Err(self.error("`return' used outside function context"));
                }
                self.next()?;
                let mut value = None;
                if !self.matches(&[Tok::Newline, Tok::Semicolon, Tok::RBrace, Tok::Eof]) {
                    value = Some(self.expr()?);
                }
                Stmt::Return(value)
            }
            Tok::LBrace => Stmt::Block(self.stmts_brace()?),
            _ => self.simple_stmt()?,
        };
        // Statements end at `;`, a newline or `}` (or after a `}` or `;` that ended
        // the statement itself).
        if !self.matches(&[Tok::Newline, Tok::Semicolon, Tok::RBrace, Tok::Eof])
            && !matches!(self.prev_tok, Tok::Newline | Tok::Semicolon | Tok::RBrace)
        {
            return Err(self.error("syntax error"));
        }
        while self.matches(&[Tok::Newline, Tok::Semicolon]) {
            self.next()?;
        }
        Ok(s)
    }

    fn loop_stmts(&mut self) -> PResult<Vec<Stmt>> {
        self.loop_depth += 1;
        let r = self.stmts();
        self.loop_depth -= 1;
        r
    }

    fn function(&mut self) -> PResult<Function> {
        if self.in_func {
            return Err(self.error("syntax error"));
        }
        self.next()?;
        // gawk allows a user function named like a builtin? No: an error.
        let (name, pos) = self.expect_name()?;
        self.expect(Tok::LParen)?;
        let mut params: Vec<Name> = Vec::new();
        let mut first = true;
        while self.tok != Tok::RParen {
            if !first {
                self.comma_newlines()?;
            }
            first = false;
            let param = self.val.clone();
            if self.tok == Tok::Name && param == name {
                return Err(self.error(&format!(
                    "function `{}': cannot use function name as parameter name",
                    String::from_utf8_lossy(&name)
                )));
            }
            if self.tok == Tok::Name && params.contains(&param) {
                return Err(self.error(&format!(
                    "function `{}': parameter #{} duplicates parameter #{}",
                    String::from_utf8_lossy(&name),
                    params.len() + 1,
                    params.iter().position(|p| *p == param).unwrap() + 1
                )));
            }
            self.expect(Tok::Name)?;
            params.push(param);
            self.optional_newlines()?;
        }
        self.expect(Tok::RParen)?;
        self.optional_newlines()?;
        self.in_func = true;
        let body = self.stmts_brace();
        self.in_func = false;
        Ok(Function {
            name,
            params,
            body: body?,
            pos,
        })
    }

    /// Comma-separated expressions: print arguments, a call's arguments, an index.
    fn expr_list(&mut self, ctx: Ctx) -> PResult<Vec<Expr>> {
        let mut exprs = Vec::new();
        let mut first = true;
        while !self.matches(&[
            Tok::Newline,
            Tok::Semicolon,
            Tok::RBrace,
            Tok::RBracket,
            Tok::RParen,
            Tok::Greater,
            Tok::Pipe,
            Tok::Append,
            Tok::Eof,
        ]) {
            if !first {
                self.comma_newlines()?;
            }
            first = false;
            exprs.push(self.expr_ctx(ctx)?);
        }
        Ok(exprs)
    }

    fn expr(&mut self) -> PResult<Expr> {
        self.expr_ctx(Ctx::Full)
    }

    fn expr_ctx(&mut self, ctx: Ctx) -> PResult<Expr> {
        self.assign(ctx)
    }

    /// `cond [| getline [lvalue]]`, for the full grammar only (getline's special case:
    /// the left side is parsed before the `|` says it is a command).
    fn getline_or_cond(&mut self, ctx: Ctx) -> PResult<Expr> {
        if ctx == Ctx::Print {
            return self.cond(ctx);
        }
        self.pending_getline_left = None;
        let mut left = self.cond(ctx)?;
        while self.tok == Tok::Pipe {
            self.pending_getline_left = Some(left);
            left = self.cond(ctx)?;
        }
        Ok(left)
    }

    fn assign(&mut self, ctx: Ctx) -> PResult<Expr> {
        let left_pos = self.pos;
        let expr = self.getline_or_cond(ctx)?;
        let op = match self.tok {
            Tok::Assign => None,
            Tok::AddAssign => Some(BinOp::Add),
            Tok::SubAssign => Some(BinOp::Sub),
            Tok::MulAssign => Some(BinOp::Mul),
            Tok::DivAssign => Some(BinOp::Div),
            Tok::ModAssign => Some(BinOp::Mod),
            Tok::PowAssign => Some(BinOp::Pow),
            _ => return Ok(expr),
        };
        self.next()?;
        let right = self.assign(ctx)?;
        if !expr.is_lvalue() {
            // `1 && x = 1`: assignments bind looser than binary operators, but
            // onetrueawk, gawk and mawk take this for logical, match and comparison
            // operators (goawk's partial backtracking).
            if let Expr::Binary(l, bop, r) = expr
                && r.is_lvalue()
                && matches!(
                    bop,
                    BinOp::And
                        | BinOp::Or
                        | BinOp::Match
                        | BinOp::NotMatch
                        | BinOp::Eq
                        | BinOp::Ne
                        | BinOp::Lt
                        | BinOp::Le
                        | BinOp::Gt
                        | BinOp::Ge
                )
            {
                return Ok(Expr::Binary(l, bop, Box::new(make_assign(*r, op, right))));
            }
            return Err(ParseError {
                pos: left_pos,
                msg: "syntax error".into(),
            });
        }
        Ok(make_assign(expr, op, right))
    }

    fn cond(&mut self, ctx: Ctx) -> PResult<Expr> {
        let expr = self.or(ctx)?;
        if self.tok == Tok::Question {
            self.next()?;
            self.optional_newlines()?;
            let t = self.expr_ctx(ctx)?;
            self.optional_newlines()?;
            self.expect(Tok::Colon)?;
            self.optional_newlines()?;
            let f = self.expr_ctx(ctx)?;
            return Ok(Expr::Cond(Box::new(expr), Box::new(t), Box::new(f)));
        }
        Ok(expr)
    }

    fn or(&mut self, ctx: Ctx) -> PResult<Expr> {
        let mut expr = self.and(ctx)?;
        while self.tok == Tok::Or {
            self.next()?;
            self.optional_newlines()?;
            let right = self.and(ctx)?;
            expr = Expr::Binary(Box::new(expr), BinOp::Or, Box::new(right));
        }
        Ok(expr)
    }

    fn and(&mut self, ctx: Ctx) -> PResult<Expr> {
        let mut expr = self.in_(ctx)?;
        while self.tok == Tok::And {
            self.next()?;
            self.optional_newlines()?;
            let right = self.in_(ctx)?;
            expr = Expr::Binary(Box::new(expr), BinOp::And, Box::new(right));
        }
        Ok(expr)
    }

    fn in_(&mut self, ctx: Ctx) -> PResult<Expr> {
        let mut expr = self.match_(ctx)?;
        while self.tok == Tok::In {
            self.next()?;
            let (name, pos) = self.expect_name()?;
            expr = Expr::In(vec![expr], name, pos);
        }
        Ok(expr)
    }

    fn match_(&mut self, ctx: Ctx) -> PResult<Expr> {
        let mut expr = self.compare(ctx)?;
        // Left-associative, as in gawk: `a ~ b ~ c`.
        while self.matches(&[Tok::Match, Tok::NotMatch]) {
            let op = if self.tok == Tok::Match {
                BinOp::Match
            } else {
                BinOp::NotMatch
            };
            self.next()?;
            let right = if self.matches(&[Tok::Div, Tok::DivAssign]) {
                // A regex literal operand, possibly concatenated? No: a lone regex.
                Expr::RegexArg(self.next_regex()?)
            } else {
                self.compare(ctx)?
            };
            expr = Expr::Binary(Box::new(expr), op, Box::new(right));
        }
        Ok(expr)
    }

    fn compare(&mut self, ctx: Ctx) -> PResult<Expr> {
        let expr = self.concat(ctx)?;
        let op = match self.tok {
            Tok::Equals => BinOp::Eq,
            Tok::NotEquals => BinOp::Ne,
            Tok::Less => BinOp::Lt,
            Tok::Lte => BinOp::Le,
            Tok::Gte => BinOp::Ge,
            Tok::Greater if ctx == Ctx::Full => BinOp::Gt,
            _ => return Ok(expr),
        };
        self.next()?;
        let right = self.concat(ctx)?;
        Ok(Expr::Binary(Box::new(expr), op, Box::new(right)))
    }

    fn concat(&mut self, ctx: Ctx) -> PResult<Expr> {
        let mut expr = self.add(ctx)?;
        while matches!(
            self.tok,
            Tok::Dollar
                | Tok::Not
                | Tok::Name
                | Tok::Number
                | Tok::Str
                | Tok::LParen
                | Tok::Incr
                | Tok::Decr
                | Tok::Func(_)
        ) {
            let right = self.add(ctx)?;
            expr = Expr::Binary(Box::new(expr), BinOp::Concat, Box::new(right));
        }
        Ok(expr)
    }

    fn add(&mut self, ctx: Ctx) -> PResult<Expr> {
        let mut expr = self.mul(ctx)?;
        while self.matches(&[Tok::Add, Tok::Sub]) {
            let op = if self.tok == Tok::Add {
                BinOp::Add
            } else {
                BinOp::Sub
            };
            self.next()?;
            let right = self.mul(ctx)?;
            expr = Expr::Binary(Box::new(expr), op, Box::new(right));
        }
        Ok(expr)
    }

    fn mul(&mut self, ctx: Ctx) -> PResult<Expr> {
        let mut expr = self.pow(ctx)?;
        while self.matches(&[Tok::Mul, Tok::Div, Tok::Mod]) {
            let op = match self.tok {
                Tok::Mul => BinOp::Mul,
                Tok::Div => BinOp::Div,
                _ => BinOp::Mod,
            };
            self.next()?;
            let right = self.pow(ctx)?;
            expr = Expr::Binary(Box::new(expr), op, Box::new(right));
        }
        Ok(expr)
    }

    fn pow(&mut self, ctx: Ctx) -> PResult<Expr> {
        // Right-associative; a unary minus after `^` binds tighter (`2^-1`).
        let expr = self.post_incr(ctx)?;
        if self.tok == Tok::Pow {
            self.next()?;
            let right = if self.matches(&[Tok::Sub, Tok::Add, Tok::Not]) {
                self.unary(ctx)?
            } else {
                self.pow(ctx)?
            };
            return Ok(Expr::Binary(Box::new(expr), BinOp::Pow, Box::new(right)));
        }
        Ok(expr)
    }

    fn unary(&mut self, ctx: Ctx) -> PResult<Expr> {
        let op = match self.tok {
            Tok::Not => UnOp::Not,
            Tok::Sub => UnOp::Neg,
            _ => UnOp::Plus,
        };
        self.next()?;
        let value = if self.matches(&[Tok::Sub, Tok::Add, Tok::Not]) {
            self.unary(ctx)?
        } else {
            self.pow(ctx)?
        };
        Ok(Expr::Unary(op, Box::new(value)))
    }

    fn post_incr(&mut self, ctx: Ctx) -> PResult<Expr> {
        let expr = self.primary(ctx)?;
        if self.matches(&[Tok::Incr, Tok::Decr]) && expr.is_lvalue() {
            let decr = self.tok == Tok::Decr;
            self.next()?;
            return Ok(Expr::Incr {
                expr: Box::new(expr),
                decr,
                pre: false,
            });
        }
        Ok(expr)
    }

    fn primary(&mut self, ctx: Ctx) -> PResult<Expr> {
        if let Some(left) = self.pending_getline_left.take() {
            self.expect(Tok::Pipe)?;
            self.expect(Tok::Getline)?;
            let target = self.optional_lvalue()?;
            return Ok(Expr::Getline {
                src: GetlineSrc::Cmd,
                target: target.map(Box::new),
                from: Some(Box::new(left)),
            });
        }
        match self.tok {
            Tok::Number => {
                let n = parse_number(&self.val);
                self.next()?;
                Ok(Expr::Num(n))
            }
            Tok::Str => {
                let s = std::mem::take(&mut self.val);
                self.next()?;
                Ok(Expr::Str(s))
            }
            Tok::Div | Tok::DivAssign => Ok(Expr::Regex(self.next_regex()?)),
            Tok::Dollar => {
                self.next()?;
                // `$x++` is `($x)++`; `$-1`, `$++i`, `$!x` take a unary operand.
                let index = if self.matches(&[Tok::Sub, Tok::Add, Tok::Not]) {
                    self.unary(ctx)?
                } else {
                    self.primary(ctx)?
                };
                let mut expr = Expr::Field(Box::new(index));
                if self.matches(&[Tok::Incr, Tok::Decr]) {
                    let decr = self.tok == Tok::Decr;
                    self.next()?;
                    expr = Expr::Incr {
                        expr: Box::new(expr),
                        decr,
                        pre: false,
                    };
                }
                Ok(expr)
            }
            Tok::Not | Tok::Add | Tok::Sub => self.unary(ctx),
            Tok::Incr | Tok::Decr => {
                let decr = self.tok == Tok::Decr;
                self.next()?;
                let pos = self.pos;
                match self.optional_lvalue()? {
                    Some(e) => Ok(Expr::Incr {
                        expr: Box::new(e),
                        decr,
                        pre: true,
                    }),
                    None => Err(ParseError {
                        pos,
                        msg: "syntax error".into(),
                    }),
                }
            }
            Tok::Name => {
                let (name, pos) = self.expect_name()?;
                if self.tok == Tok::LBracket {
                    self.next()?;
                    let index = self.expr_list(Ctx::Full)?;
                    if index.is_empty() {
                        return Err(self.error("syntax error"));
                    }
                    self.expect(Tok::RBracket)?;
                    Ok(Expr::Index(name, pos, index))
                } else if self.tok == Tok::LParen && !self.lexer.had_space() {
                    self.user_call(name, pos)
                } else {
                    Ok(Expr::Var(name, pos))
                }
            }
            Tok::LParen => {
                let paren_pos = self.pos;
                self.next()?;
                let mut exprs = self.expr_list(Ctx::Full)?;
                match exprs.len() {
                    0 => Err(self.error("syntax error")),
                    1 => {
                        self.expect(Tok::RParen)?;
                        Ok(Expr::Group(Box::new(exprs.pop().unwrap())))
                    }
                    _ => {
                        self.expect(Tok::RParen)?;
                        if self.tok == Tok::In {
                            self.next()?;
                            let (name, pos) = self.expect_name()?;
                            return Ok(Expr::In(exprs, name, pos));
                        }
                        // Only print and printf take this (their arguments in
                        // parentheses).
                        self.multis.push(paren_pos);
                        Ok(Expr::Multi(exprs, paren_pos))
                    }
                }
            }
            Tok::Getline => {
                self.next()?;
                let target = self.optional_lvalue()?;
                let mut from = None;
                let mut src = GetlineSrc::Main;
                if self.tok == Tok::Less {
                    self.next()?;
                    from = Some(Box::new(self.primary_concatless(ctx)?));
                    src = GetlineSrc::File;
                }
                Ok(Expr::Getline {
                    src,
                    target: target.map(Box::new),
                    from,
                })
            }
            Tok::Func(b) => self.builtin_call(b),
            _ => Err(self.error("syntax error")),
        }
    }

    /// The file of `getline < file`: a primary (with `$`), not a concatenation.
    fn primary_concatless(&mut self, ctx: Ctx) -> PResult<Expr> {
        self.primary(ctx)
    }

    fn builtin_call(&mut self, b: Builtin) -> PResult<Expr> {
        use Builtin as B;
        self.next()?;
        match b {
            B::Sub | B::Gsub => {
                self.expect(Tok::LParen)?;
                let regex = self.regex_str()?;
                self.comma_newlines()?;
                let repl = self.expr()?;
                let mut args = vec![regex, repl];
                if self.tok == Tok::Comma {
                    self.comma_newlines()?;
                    let pos = self.pos;
                    let target = self.expr()?;
                    if !target.is_lvalue() {
                        return Err(ParseError {
                            pos,
                            msg: format!(
                                "{}: third argument is not a changeable object",
                                if b == B::Sub { "sub" } else { "gsub" }
                            ),
                        });
                    }
                    args.push(target);
                }
                self.expect(Tok::RParen)?;
                Ok(Expr::Call(b, args))
            }
            B::Gensub => {
                self.expect(Tok::LParen)?;
                let regex = self.regex_str()?;
                self.comma_newlines()?;
                let mut args = vec![regex, self.expr()?];
                self.comma_newlines()?;
                args.push(self.expr()?);
                if self.tok == Tok::Comma {
                    self.comma_newlines()?;
                    args.push(self.expr()?);
                }
                self.expect(Tok::RParen)?;
                Ok(Expr::Call(b, args))
            }
            B::Split => {
                self.expect(Tok::LParen)?;
                let s = self.expr()?;
                self.comma_newlines()?;
                let (name, pos) = self.expect_name()?;
                let mut args = vec![s, Expr::Var(name, pos)];
                if self.tok == Tok::Comma {
                    self.comma_newlines()?;
                    args.push(self.regex_str()?);
                }
                self.expect(Tok::RParen)?;
                Ok(Expr::Call(b, args))
            }
            B::Match => {
                self.expect(Tok::LParen)?;
                let s = self.expr()?;
                self.comma_newlines()?;
                let mut args = vec![s, self.regex_str()?];
                if self.tok == Tok::Comma {
                    // gawk: match(s, re, arr).
                    self.comma_newlines()?;
                    let (name, pos) = self.expect_name()?;
                    args.push(Expr::Var(name, pos));
                }
                self.expect(Tok::RParen)?;
                Ok(Expr::Call(b, args))
            }
            B::Length => {
                // `length` without parentheses is `length($0)`.
                let mut args = Vec::new();
                if self.tok == Tok::LParen {
                    self.next()?;
                    if self.tok != Tok::RParen {
                        args.push(self.expr()?);
                    }
                    self.expect(Tok::RParen)?;
                }
                Ok(Expr::Call(b, args))
            }
            _ => {
                // The others: a fixed or optional count of plain arguments.
                let (min, max) = match b {
                    B::Rand | B::Systime => (0, 0),
                    B::Srand | B::Fflush => (0, 1),
                    B::Close => (1, 2),
                    B::Cos
                    | B::Sin
                    | B::Exp
                    | B::Log
                    | B::Sqrt
                    | B::Int
                    | B::Tolower
                    | B::Toupper
                    | B::System => (1, 1),
                    B::Atan2 | B::Index => (2, 2),
                    B::Substr => (2, 3),
                    B::Sprintf => (1, usize::MAX),
                    B::Strftime => (0, 3),
                    B::Mktime => (1, 2),
                    _ => unreachable!(),
                };
                if self.tok != Tok::LParen && min == 0 && max <= 1 && b == B::Fflush {
                    // `fflush` alone (old gawk): flush everything.
                    return Ok(Expr::Call(b, Vec::new()));
                }
                self.expect(Tok::LParen)?;
                let mut args = Vec::new();
                while self.tok != Tok::RParen {
                    if !args.is_empty() {
                        self.comma_newlines()?;
                    }
                    args.push(self.expr()?);
                }
                if args.len() < min || args.len() > max {
                    return Err(self.error(&format!(
                        "{} called with {} arguments",
                        builtin_name(b),
                        args.len()
                    )));
                }
                self.expect(Tok::RParen)?;
                Ok(Expr::Call(b, args))
            }
        }
    }

    fn optional_lvalue(&mut self) -> PResult<Option<Expr>> {
        match self.tok {
            Tok::Name => {
                if self.lexer.peek_byte() == b'(' {
                    // A user function call, not an lvalue.
                    return Ok(None);
                }
                let (name, pos) = self.expect_name()?;
                if self.tok == Tok::LBracket {
                    self.next()?;
                    let index = self.expr_list(Ctx::Full)?;
                    if index.is_empty() {
                        return Err(self.error("syntax error"));
                    }
                    self.expect(Tok::RBracket)?;
                    return Ok(Some(Expr::Index(name, pos, index)));
                }
                Ok(Some(Expr::Var(name, pos)))
            }
            Tok::Dollar => {
                self.next()?;
                let index = if self.matches(&[Tok::Sub, Tok::Add, Tok::Not]) {
                    self.unary(Ctx::Full)?
                } else {
                    self.primary(Ctx::Full)?
                };
                Ok(Some(Expr::Field(Box::new(index))))
            }
            _ => Ok(None),
        }
    }

    /// A `/regex/` literal or an expression (the regex arguments of `~`, sub, split...).
    fn regex_str(&mut self) -> PResult<Expr> {
        if self.matches(&[Tok::Div, Tok::DivAssign]) {
            return Ok(Expr::RegexArg(self.next_regex()?));
        }
        self.expr()
    }

    fn comma_newlines(&mut self) -> PResult<()> {
        self.expect(Tok::Comma)?;
        self.optional_newlines()
    }

    fn optional_newlines(&mut self) -> PResult<()> {
        while self.tok == Tok::Newline {
            self.next()?;
        }
        Ok(())
    }

    fn next(&mut self) -> PResult<()> {
        self.prev_tok = self.tok;
        let (pos, tok, val) = self.lexer.scan();
        self.pos = pos;
        self.tok = tok;
        if tok == Tok::Illegal {
            return Err(ParseError {
                pos,
                msg: String::from_utf8_lossy(&val).into_owned(),
            });
        }
        self.val = val;
        Ok(())
    }

    /// The regex after a `/` or `/=` token.
    fn next_regex(&mut self) -> PResult<Vec<u8>> {
        let (pos, tok, val) = self.lexer.scan_regex();
        self.pos = pos;
        self.tok = tok;
        if tok == Tok::Illegal {
            return Err(ParseError {
                pos,
                msg: String::from_utf8_lossy(&val).into_owned(),
            });
        }
        self.next()?;
        Ok(val)
    }

    fn expect(&mut self, tok: Tok) -> PResult<()> {
        if self.tok != tok {
            return Err(self.error("syntax error"));
        }
        self.next()
    }

    fn expect_name(&mut self) -> PResult<(Name, Pos)> {
        let pos = self.pos;
        let name = std::mem::take(&mut self.val);
        if self.tok != Tok::Name {
            self.val = name;
            return Err(self.error("syntax error"));
        }
        self.next()?;
        Ok((name, pos))
    }

    fn matches(&self, toks: &[Tok]) -> bool {
        toks.contains(&self.tok)
    }

    fn error(&self, msg: &str) -> ParseError {
        // gawk's words for a program that ends too early.
        let msg = if msg == "syntax error" && matches!(self.tok, Tok::Eof | Tok::Newline) {
            "unexpected newline or end of string"
        } else {
            msg
        };
        ParseError {
            pos: self.pos,
            msg: msg.into(),
        }
    }

    fn user_call(&mut self, name: Name, pos: Pos) -> PResult<Expr> {
        self.expect(Tok::LParen)?;
        let mut args = Vec::new();
        while !self.matches(&[Tok::Newline, Tok::RParen]) {
            if !args.is_empty() {
                self.comma_newlines()?;
            }
            args.push(self.expr()?);
        }
        self.expect(Tok::RParen)?;
        Ok(Expr::UserCall(name, pos, args))
    }
}

fn make_assign(left: Expr, op: Option<BinOp>, right: Expr) -> Expr {
    match op {
        None => Expr::Assign(Box::new(left), Box::new(right)),
        Some(op) => Expr::AugAssign(Box::new(left), op, Box::new(right)),
    }
}

pub fn builtin_name(b: Builtin) -> &'static str {
    use Builtin as B;
    match b {
        B::Atan2 => "atan2",
        B::Close => "close",
        B::Cos => "cos",
        B::Exp => "exp",
        B::Fflush => "fflush",
        B::Gsub => "gsub",
        B::Index => "index",
        B::Int => "int",
        B::Length => "length",
        B::Log => "log",
        B::Match => "match",
        B::Rand => "rand",
        B::Sin => "sin",
        B::Split => "split",
        B::Sprintf => "sprintf",
        B::Sqrt => "sqrt",
        B::Srand => "srand",
        B::Sub => "sub",
        B::Substr => "substr",
        B::System => "system",
        B::Tolower => "tolower",
        B::Toupper => "toupper",
        B::Gensub => "gensub",
        B::Systime => "systime",
        B::Strftime => "strftime",
        B::Mktime => "mktime",
    }
}

/// A numeric literal of the source: decimal, or gawk's `0x` hex.
fn parse_number(s: &[u8]) -> f64 {
    if s.len() > 2 && s[0] == b'0' && (s[1] == b'x' || s[1] == b'X') {
        let mut v = 0f64;
        for &c in &s[2..] {
            v = v * 16.0 + (c as char).to_digit(16).unwrap_or(0) as f64;
        }
        return v;
    }
    std::str::from_utf8(s)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0.0)
}
