//! The typed AST of the builtin subset.

use crate::diag::Span;

/// The types of the subset. `IntLit`/`NumLit` are the types of numeric literals (and of
/// arithmetic on literals only), which adapt to the numeric type they meet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Type {
    Value,
    Obj,
    Fn,
    Str,
    Sym,
    Bool,
    I32,
    U32,
    F64,
    Void,
    Never,
    Undefined,
    Null,
    IntLit(f64),
    NumLit,
    /// The type of an expression that already has an error: compatible with
    /// everything, so one mistake gives one message.
    Error,
}

impl Type {
    /// The type names usable in annotations.
    pub fn from_name(name: &str) -> Option<Type> {
        Some(match name {
            "Value" => Type::Value,
            "Obj" => Type::Obj,
            "Fn" => Type::Fn,
            "Str" => Type::Str,
            "Sym" => Type::Sym,
            "boolean" => Type::Bool,
            "i32" => Type::I32,
            "u32" => Type::U32,
            "f64" => Type::F64,
            "void" => Type::Void,
            "never" => Type::Never,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Type::Value => "Value",
            Type::Obj => "Obj",
            Type::Fn => "Fn",
            Type::Str => "Str",
            Type::Sym => "Sym",
            Type::Bool => "boolean",
            Type::I32 => "i32",
            Type::U32 => "u32",
            Type::F64 => "f64",
            Type::Void => "void",
            Type::Never => "never",
            Type::Undefined => "undefined",
            Type::Null => "null",
            Type::IntLit(_) => "integer literal",
            Type::NumLit => "number literal",
            Type::Error => "<error>",
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            Type::I32 | Type::U32 | Type::F64 | Type::IntLit(_) | Type::NumLit
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: TypeRef,
    pub optional: bool,
    /// `...name: T[]`; `ty` is the element type. In a builtin the local is an array.
    pub rest: bool,
    pub span: Span,
}

/// A parsed JSDoc tag: `@name rest-of-line`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub name: String,
    pub value: String,
}

/// `declare function` in intrinsics.d.ts.
#[derive(Debug, Clone, PartialEq)]
pub struct IntrinsicDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: TypeRef,
    pub tags: Vec<Tag>,
    pub span: Span,
}

/// A builtin function in builtins/*.ts.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub this_ty: Option<TypeRef>,
    pub params: Vec<Param>,
    pub ret: TypeRef,
    pub body: Block,
    pub tags: Vec<Tag>,
    pub span: Span,
    /// Span of the name, for messages.
    pub name_span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Decl {
        kind: DeclKind,
        name: String,
        ty: TypeRef,
        init: Expr,
        span: Span,
    },
    Assign {
        target: String,
        target_span: Span,
        op: AssignOp,
        value: Expr,
        span: Span,
    },
    /// `x++`, `x--`, `++x`, `--x` as statements.
    Update {
        target: String,
        inc: bool,
        span: Span,
    },
    Expr(Expr),
    If {
        cond: Expr,
        then: Box<Stmt>,
        els: Option<Box<Stmt>>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Box<Stmt>,
        span: Span,
    },
    DoWhile {
        body: Box<Stmt>,
        cond: Expr,
        span: Span,
    },
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        update: Option<Box<Stmt>>,
        body: Box<Stmt>,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    Throw {
        value: Expr,
        span: Span,
    },
    Try {
        body: Block,
        catch: Option<Catch>,
        finally: Option<Block>,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    Block(Block),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Const,
    Let,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Catch {
    /// The binding, if any (always of type Value).
    pub name: Option<(String, Span)>,
    pub body: Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

impl AssignOp {
    pub fn as_str(self) -> &'static str {
        match self {
            AssignOp::Assign => "=",
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
            AssignOp::Div => "/=",
            AssignOp::Rem => "%=",
            AssignOp::BitAnd => "&=",
            AssignOp::BitOr => "|=",
            AssignOp::BitXor => "^=",
            AssignOp::Shl => "<<=",
            AssignOp::Shr => ">>=",
        }
    }

    /// The binary operator a compound assignment applies.
    pub fn binary(self) -> Option<BinOp> {
        Some(match self {
            AssignOp::Assign => return None,
            AssignOp::Add => BinOp::Add,
            AssignOp::Sub => BinOp::Sub,
            AssignOp::Mul => BinOp::Mul,
            AssignOp::Div => BinOp::Div,
            AssignOp::Rem => BinOp::Rem,
            AssignOp::BitAnd => BinOp::BitAnd,
            AssignOp::BitOr => BinOp::BitOr,
            AssignOp::BitXor => BinOp::BitXor,
            AssignOp::Shl => BinOp::Shl,
            AssignOp::Shr => BinOp::Shr,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    Lt,
    Le,
    Gt,
    Ge,
    StrictEq,
    StrictNe,
    And,
    Or,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::UShr => ">>>",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::StrictEq => "===",
            BinOp::StrictNe => "!==",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    /// JS precedence (higher binds tighter), used by the parser and the emitter.
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::BitOr => 3,
            BinOp::BitXor => 4,
            BinOp::BitAnd => 5,
            BinOp::StrictEq | BinOp::StrictNe => 6,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 7,
            BinOp::Shl | BinOp::Shr | BinOp::UShr => 8,
            BinOp::Add | BinOp::Sub => 9,
            BinOp::Mul | BinOp::Div | BinOp::Rem => 10,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    BitNot,
}

impl UnOp {
    pub fn as_str(self) -> &'static str {
        match self {
            UnOp::Not => "!",
            UnOp::Neg => "-",
            UnOp::BitNot => "~",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Ident(String),
    This,
    Undefined,
    Null,
    Bool(bool),
    Num(f64, String),
    Str(Vec<u16>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Call {
        callee: String,
        callee_span: Span,
        args: Vec<Expr>,
    },
    Paren(Box<Expr>),
    /// `new.target` (only in `@constructor` builtins).
    NewTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

/// One parsed builtins/*.ts file.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub file: u32,
    pub functions: Vec<Function>,
}

/// Parsed intrinsics.d.ts.
#[derive(Debug, Clone, PartialEq)]
pub struct Declarations {
    pub intrinsics: Vec<IntrinsicDecl>,
}
