use nanocode_macro_support::Ident;

pub trait GetIdent {
    fn get_ident(&self) -> Option<&Ident>;
}
