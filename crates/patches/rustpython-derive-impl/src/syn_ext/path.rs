use crate::syn_ext::ident::GetIdent;
use syn::{Ident, Path};

pub trait GetPath {
    fn get_path(&self) -> Option<&Path>;
}

impl<T> GetIdent for T
where
    T: GetPath,
{
    fn get_ident(&self) -> Option<&Ident> {
        self.get_path().and_then(|p| p.get_ident())
    }
}
