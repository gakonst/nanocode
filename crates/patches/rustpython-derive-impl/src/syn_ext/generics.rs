use crate::syn_ext::ext::GetIdent;
use syn::{GenericParam, Ident};

impl GetIdent for GenericParam {
    fn get_ident(&self) -> Option<&Ident> {
        match self {
            Self::Type(t) => Some(&t.ident),
            Self::Lifetime(l) => Some(&l.lifetime.ident),
            Self::Const(c) => Some(&c.ident),
        }
    }
}
