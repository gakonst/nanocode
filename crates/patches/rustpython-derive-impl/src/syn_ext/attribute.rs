use crate::syn_ext::ident::GetIdent;
use crate::syn_ext::meta::{self, MetaExt};
use crate::syn_ext::meta::{Meta1, MetaList1};
use nanocode_macro_support::{Attribute, Ident, Punctuated, Result, Span};

impl GetIdent for Attribute {
    fn get_ident(&self) -> Option<&Ident> {
        self.path().get_ident()
    }
}
pub trait AttributeExt {
    fn from_meta<M>(meta: M) -> Self
    where
        M: IntoAttribute;

    fn parse_meta(&self) -> Result<Meta1>;

    fn try_meta_mut<F, R>(&mut self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Meta1) -> Result<R>;

    fn promoted_list(&self) -> Result<MetaList1>;

    fn try_promoted_list_mut<F, R>(&mut self, paren: Span, f: F) -> Result<R>
    where
        F: FnOnce(&mut MetaList1) -> Result<R>;
}
impl AttributeExt for Attribute {
    fn from_meta<M>(meta: M) -> Self
    where
        M: IntoAttribute,
    {
        meta.into_attribute()
    }

    fn parse_meta(&self) -> Result<Meta1> {
        use std::convert::TryInto;
        self.meta.clone().try_into()
    }

    fn try_meta_mut<F, R>(&mut self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Meta1) -> Result<R>,
    {
        let mut meta = self.parse_meta()?;
        let result = f(&mut meta);
        *self = Self::from_meta(meta);
        result
    }

    fn promoted_list(&self) -> Result<MetaList1> {
        match self.parse_meta()? {
            Meta1::Path(path) => Ok(MetaList1 {
                path,
                paren_span: Span::call_site(),
                nested: Punctuated::new(),
            }),
            Meta1::List(metalist) => Ok(metalist),
            other => Err(meta::err_promote_to_list(&other)),
        }
    }

    fn try_promoted_list_mut<F, R>(&mut self, paren: Span, f: F) -> Result<R>
    where
        F: FnOnce(&mut MetaList1) -> Result<R>,
    {
        self.try_meta_mut(|meta| {
            let metalist = meta.promote_to_list(paren)?;
            f(metalist)
        })
    }
}
pub trait AttributeIteratorExt {
    fn doc(self) -> Option<String>;
}
impl<'a, I> AttributeIteratorExt for I
where
    I: std::iter::IntoIterator<Item = &'a Attribute>,
{
    fn doc(self) -> Option<String> {
        let items: Vec<_> = self
            .into_iter()
            .filter_map(|attr| attr.parse_meta().ok().and_then(|m| m.doc().ok()))
            .collect();
        if items.is_empty() {
            None
        } else {
            Some(items.join("\n"))
        }
    }
}

pub trait IntoAttribute {
    fn into_attribute(self) -> Attribute;
}

impl IntoAttribute for Meta1 {
    fn into_attribute(self) -> Attribute {
        parse_quote!( #[ #self ] )
    }
}

impl IntoAttribute for MetaList1 {
    fn into_attribute(self) -> Attribute {
        Meta1::List(self).into_attribute()
    }
}
