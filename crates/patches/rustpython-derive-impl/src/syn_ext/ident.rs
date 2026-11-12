use syn::Ident;

pub trait GetIdent {
    fn get_ident(&self) -> Option<&Ident>;
}
