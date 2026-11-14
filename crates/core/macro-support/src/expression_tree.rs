use proc_macro::{Delimiter, Group, Spacing, Span, TokenTree};

use crate::attribute::Attribute;
use crate::buffer::{Parse, ParseBuffer, ParseStream};
use crate::error::Result;
use crate::expression::ExprPath;
use crate::literal::{Lit, LitBool};
use crate::tokens::{ToTokens, Tokens, push_group, push_punct};

#[derive(Clone)]
pub enum Expr {
    Group(ExprGroup),
    Lit(ExprLit),
    Paren(ExprParen),
    Path(ExprPath),
    Unary(ExprUnary),
    Verbatim(Tokens),
}

#[derive(Clone)]
pub struct ExprGroup {
    pub attrs: Vec<Attribute>,
    pub group_span: Span,
    pub expr: Box<Expr>,
}

#[derive(Clone)]
pub struct ExprLit {
    pub attrs: Vec<Attribute>,
    pub lit: Lit,
}

#[derive(Clone)]
pub struct ExprParen {
    pub attrs: Vec<Attribute>,
    pub paren_span: Span,
    pub expr: Box<Expr>,
}

#[derive(Clone)]
pub struct ExprUnary {
    pub attrs: Vec<Attribute>,
    pub op: UnOp,
    pub expr: Box<Expr>,
}

#[derive(Clone, Copy)]
pub enum UnOp {
    Deref(Span),
    Not(Span),
    Neg(Span),
}

impl Expr {
    pub fn parse_until(input: ParseStream<'_>, stop: char) -> Self {
        Self::from_tokens(expression_tokens(input, stop))
    }

    pub fn from_tokens(tokens: Tokens) -> Self {
        if let Some(expression) = classify_single(tokens.trees()) {
            return expression;
        }
        if let Some(expression) = classify_unary(tokens.trees()) {
            return expression;
        }
        let buffer = ParseBuffer::new(tokens.clone());
        if let Ok(path) = ExprPath::parse(&buffer)
            && buffer.is_empty()
        {
            return Self::Path(path);
        }
        Self::Verbatim(tokens)
    }

    pub fn from_meta_value(tokens: &Tokens) -> Self {
        let buffer = ParseBuffer::new(tokens.clone());
        if buffer.peek_literal()
            && let Ok(lit) = Lit::parse(&buffer)
            && buffer.is_empty()
        {
            return Self::Lit(ExprLit { attrs: Vec::new(), lit });
        }
        Self::from_tokens(tokens.clone())
    }

    const fn is_operand(&self) -> bool {
        !matches!(self, Self::Verbatim(_))
    }
}

fn classify_single(trees: &[TokenTree]) -> Option<Expr> {
    let [tree] = trees else {
        return None;
    };
    match tree {
        TokenTree::Literal(literal) => Some(Expr::Lit(ExprLit { attrs: Vec::new(), lit: Lit::from_literal(literal.clone()) })),
        TokenTree::Ident(ident) => {
            let word = ident.to_string();
            if word != "true" && word != "false" {
                return None;
            }
            let lit = Lit::Bool(LitBool { value: word == "true", span: ident.span() });
            Some(Expr::Lit(ExprLit { attrs: Vec::new(), lit }))
        }
        TokenTree::Group(group) => classify_group(group),
        TokenTree::Punct(_) => None,
    }
}

fn classify_group(group: &Group) -> Option<Expr> {
    let inner = Tokens::from_trees(group.stream().into_iter().collect());
    match group.delimiter() {
        Delimiter::None => {
            let expr = Box::new(Expr::from_tokens(inner));
            Some(Expr::Group(ExprGroup { attrs: Vec::new(), group_span: group.span(), expr }))
        }
        Delimiter::Parenthesis => {
            let has_comma = inner.trees().iter().any(|tree| matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ','));
            if inner.is_empty() || has_comma {
                return None;
            }
            let expr = Box::new(Expr::from_tokens(inner));
            Some(Expr::Paren(ExprParen { attrs: Vec::new(), paren_span: group.span(), expr }))
        }
        Delimiter::Bracket | Delimiter::Brace => None,
    }
}

fn classify_unary(trees: &[TokenTree]) -> Option<Expr> {
    let [TokenTree::Punct(punct), rest @ ..] = trees else {
        return None;
    };
    if rest.is_empty() || punct.spacing() == Spacing::Joint {
        return None;
    }
    let op = match punct.as_char() {
        '-' => UnOp::Neg(punct.span()),
        '!' => UnOp::Not(punct.span()),
        '*' => UnOp::Deref(punct.span()),
        _ => return None,
    };
    let expr = Expr::from_tokens(Tokens::from_trees(rest.to_vec()));
    if !expr.is_operand() {
        return None;
    }
    Some(Expr::Unary(ExprUnary { attrs: Vec::new(), op, expr: Box::new(expr) }))
}

fn follows_path_separator(trees: &[TokenTree]) -> bool {
    matches!(
        trees,
        [.., TokenTree::Punct(first), TokenTree::Punct(second)]
            if first.as_char() == ':' && first.spacing() == Spacing::Joint && second.as_char() == ':'
    )
}

fn follows_joint(trees: &[TokenTree], character: char) -> bool {
    matches!(trees.last(), Some(TokenTree::Punct(punct)) if punct.as_char() == character && punct.spacing() == Spacing::Joint)
}

fn starts_operand(trees: &[TokenTree]) -> bool {
    match trees.last() {
        None | Some(TokenTree::Punct(_)) => true,
        Some(TokenTree::Ident(ident)) => ident.to_string() == "move",
        Some(TokenTree::Group(_) | TokenTree::Literal(_)) => false,
    }
}

pub fn expression_tokens(input: ParseStream<'_>, stop: char) -> Tokens {
    let mut trees: Vec<TokenTree> = Vec::new();
    let mut angle_depth = 0usize;
    let mut in_closure_parameters = false;
    while let Some(tree) = input.peek_tree(0) {
        if let TokenTree::Punct(punct) = tree {
            let character = punct.as_char();
            if character == stop && angle_depth == 0 && !in_closure_parameters {
                break;
            }
            match character {
                '<' if angle_depth > 0 || trees.is_empty() || follows_path_separator(&trees) => angle_depth += 1,
                '>' if angle_depth > 0 && !follows_joint(&trees, '-') => angle_depth -= 1,
                '|' if in_closure_parameters => in_closure_parameters = false,
                '|' if starts_operand(&trees) => in_closure_parameters = true,
                _ => {}
            }
        }
        trees.push(tree.clone());
        input.next_tree();
    }
    Tokens::from_trees(trees)
}

impl Parse for Expr {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let tokens = expression_tokens(input, ',');
        if tokens.is_empty() {
            return Err(input.error("expected an expression"));
        }
        Ok(Self::from_tokens(tokens))
    }
}

impl ToTokens for Expr {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Group(group) => group.to_tokens(tokens),
            Self::Lit(literal) => literal.to_tokens(tokens),
            Self::Paren(paren) => paren.to_tokens(tokens),
            Self::Path(path) => path.to_tokens(tokens),
            Self::Unary(unary) => unary.to_tokens(tokens),
            Self::Verbatim(verbatim) => verbatim.to_tokens(tokens),
        }
    }
}

impl ToTokens for ExprGroup {
    fn to_tokens(&self, tokens: &mut Tokens) {
        push_group(tokens, self.group_span, Delimiter::None, self.expr.to_token_stream());
    }
}

impl ToTokens for ExprLit {
    fn to_tokens(&self, tokens: &mut Tokens) {
        for attribute in &self.attrs {
            attribute.to_tokens(tokens);
        }
        self.lit.to_tokens(tokens);
    }
}

impl ToTokens for ExprParen {
    fn to_tokens(&self, tokens: &mut Tokens) {
        for attribute in &self.attrs {
            attribute.to_tokens(tokens);
        }
        push_group(tokens, self.paren_span, Delimiter::Parenthesis, self.expr.to_token_stream());
    }
}

impl ToTokens for ExprUnary {
    fn to_tokens(&self, tokens: &mut Tokens) {
        for attribute in &self.attrs {
            attribute.to_tokens(tokens);
        }
        self.op.to_tokens(tokens);
        self.expr.to_tokens(tokens);
    }
}

impl ToTokens for UnOp {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Deref(span) => push_punct(tokens, *span, "*"),
            Self::Not(span) => push_punct(tokens, *span, "!"),
            Self::Neg(span) => push_punct(tokens, *span, "-"),
        }
    }
}
