use nanocode_macro_support as syntax;
use syntax::{Delimiter, Span, ToTokens, Tokens};

fn braced(out: &mut Tokens, block: &Tokens) {
    syntax::__private::push_group(out, Span::call_site(), Delimiter::Brace, block.clone());
}

pub enum Fragment {
    Expr(Tokens),

    Block(Tokens),
}

macro_rules! quote_expr {
    ($($tt:tt)*) => {
        $crate::fragment::Fragment::Expr(quote!($($tt)*))
    }
}

macro_rules! quote_block {
    ($($tt:tt)*) => {
        $crate::fragment::Fragment::Block(quote!($($tt)*))
    }
}

pub struct Expr(pub Fragment);
impl ToTokens for Expr {
    fn to_tokens(&self, out: &mut Tokens) {
        match &self.0 {
            Fragment::Expr(expr) => expr.to_tokens(out),
            Fragment::Block(block) => {
                braced(out, block);
            }
        }
    }
}

pub struct Stmts(pub Fragment);
impl ToTokens for Stmts {
    fn to_tokens(&self, out: &mut Tokens) {
        match &self.0 {
            Fragment::Expr(expr) => expr.to_tokens(out),
            Fragment::Block(block) => block.to_tokens(out),
        }
    }
}

pub struct Match(pub Fragment);
impl ToTokens for Match {
    fn to_tokens(&self, out: &mut Tokens) {
        match &self.0 {
            Fragment::Expr(expr) => {
                expr.to_tokens(out);
                <syntax::token::Comma>::default().to_tokens(out);
            }
            Fragment::Block(block) => {
                braced(out, block);
            }
        }
    }
}

impl AsRef<Tokens> for Fragment {
    fn as_ref(&self) -> &Tokens {
        match self {
            Self::Expr(expr) => expr,
            Self::Block(block) => block,
        }
    }
}
