use crate::attribute::Attribute;
use crate::buffer::{Parse, ParseStream};
use crate::error::Result;
use crate::path::{Path, QSelf, parse_qualified_path, print_path};
use crate::tokens::{ToTokens, Tokens};

#[derive(Clone)]
pub struct ExprPath {
    pub attrs: Vec<Attribute>,
    pub qself: Option<QSelf>,
    pub path: Path,
}

impl Parse for ExprPath {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let attrs = Attribute::parse_outer(input)?;
        let (qself, path) = parse_qualified_path(input, true)?;
        Ok(Self { attrs, qself, path })
    }
}

impl ToTokens for ExprPath {
    fn to_tokens(&self, tokens: &mut Tokens) {
        for attribute in &self.attrs {
            attribute.to_tokens(tokens);
        }
        print_path(tokens, self.qself.as_ref(), &self.path);
    }
}
