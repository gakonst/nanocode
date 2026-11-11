// Structured after GoAWK's AST (internal/ast/ast.go),
// Copyright (c) 2022 Ben Hoyt, MIT License (see NOTICE).

//! The syntax tree of an awk program, as the parser builds it.

use super::lexer::{Builtin, Pos};

pub type Name = Vec<u8>;

#[derive(Debug, Default)]
pub struct Program {
    pub begin: Vec<Vec<Stmt>>,
    pub actions: Vec<Action>,
    pub end: Vec<Vec<Stmt>>,
    pub functions: Vec<Function>,
}

#[derive(Debug)]
pub struct Action {
    /// None, one (a condition) or two (a range).
    pub pattern: Vec<Expr>,
    /// `None`: no action, `{ print }`.
    pub body: Option<Vec<Stmt>>,
}

#[derive(Debug)]
pub struct Function {
    pub name: Name,
    pub params: Vec<Name>,
    pub body: Vec<Stmt>,
    pub pos: Pos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redirect {
    None,
    /// `> file`
    Write,
    /// `>> file`
    Append,
    /// `| command`
    Pipe,
}

#[derive(Debug)]
pub enum Stmt {
    Print {
        args: Vec<Expr>,
        redirect: Redirect,
        dest: Option<Expr>,
    },
    Printf {
        args: Vec<Expr>,
        redirect: Redirect,
        dest: Option<Expr>,
    },
    Expr(Expr),
    If {
        cond: Expr,
        body: Vec<Stmt>,
        else_: Vec<Stmt>,
    },
    For {
        pre: Option<Box<Stmt>>,
        cond: Option<Expr>,
        post: Option<Box<Stmt>>,
        body: Vec<Stmt>,
    },
    ForIn {
        var: Name,
        var_pos: Pos,
        array: Name,
        array_pos: Pos,
        body: Vec<Stmt>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    DoWhile {
        body: Vec<Stmt>,
        cond: Expr,
    },
    Break,
    Continue,
    Next,
    Nextfile,
    Exit(Option<Expr>),
    Return(Option<Expr>),
    Delete {
        array: Name,
        pos: Pos,
        index: Vec<Expr>,
    },
    Block(Vec<Stmt>),
    /// The statements that follow start on this source line.
    Line(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Match,
    NotMatch,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    Plus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GetlineSrc {
    /// `getline`: the main input.
    Main,
    /// `getline < file`
    File,
    /// `cmd | getline`
    Cmd,
}

#[derive(Debug)]
pub enum Expr {
    Num(f64),
    Str(Vec<u8>),
    /// A regex literal where a regex is expected (`~`, `sub`, `split`, `match`).
    RegexArg(Vec<u8>),
    /// A regex literal elsewhere: `$0 ~ /re/`.
    Regex(Vec<u8>),
    Field(Box<Expr>),
    Var(Name, Pos),
    Index(Name, Pos, Vec<Expr>),
    Assign(Box<Expr>, Box<Expr>),
    AugAssign(Box<Expr>, BinOp, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Binary(Box<Expr>, BinOp, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Incr {
        expr: Box<Expr>,
        decr: bool,
        pre: bool,
    },
    In(Vec<Expr>, Name, Pos),
    Call(Builtin, Vec<Expr>),
    UserCall(Name, Pos, Vec<Expr>),
    Getline {
        src: GetlineSrc,
        target: Option<Box<Expr>>,
        from: Option<Box<Expr>>,
    },
    Group(Box<Expr>),
    /// `(a, b)`: only valid as the arguments of print/printf or before `in`.
    Multi(Vec<Expr>, Pos),
}

impl Expr {
    pub fn is_lvalue(&self) -> bool {
        matches!(self, Expr::Var(..) | Expr::Index(..) | Expr::Field(_))
    }
}
