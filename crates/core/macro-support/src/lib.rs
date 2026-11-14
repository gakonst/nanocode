extern crate proc_macro;

mod attribute;
mod buffer;
mod data;
mod error;
mod expression;
mod generics;
mod ident;
mod literal;
mod path;
mod punctuated;
mod quote;
mod tokens;
mod ty;

pub use attribute::{
    AttrStyle, Attribute, Meta, MetaList, MetaNameValue, NestedMeta, parse_meta_path,
    parse_nested_meta,
};
pub use buffer::{
    Lookahead, Parse, ParseBuffer, ParseStream, lex_string, parse_quoted, parse_stream,
    parse_string, parse_terminated, parse_tokens, respan,
};
pub use data::{
    Data, DataEnum, DataStruct, DataUnion, DeriveInput, Field, Fields, FieldsNamed, FieldsUnnamed,
    Index, Member, Variant, Visibility,
};
pub use error::{Error, Result};
pub use expression::ExprPath;
pub use generics::{
    BoundLifetimes, ConstParam, GenericParam, Generics, ImplGenerics, LifetimeParam,
    PredicateLifetime, PredicateType, Turbofish, TypeGenerics, TypeParam, WhereClause,
    WherePredicate,
};
pub use ident::{Ident, Lifetime, is_keyword};
pub use literal::{Lit, LitBool, LitInt, LitStr, parse_string_literal};
pub use path::{
    AngleBracketedGenericArguments, AssocConst, AssocType, Constraint, GenericArgument,
    ParenthesizedGenericArguments, Path, PathArguments, PathSegment, QSelf, ReturnType,
    parse_optional, parse_qualified_path, print_path,
};
pub use proc_macro::{Delimiter, Group, Literal, Punct, Spacing, Span, TokenStream, TokenTree};
pub use punctuated::{Pair, Punctuated, Token};
pub use tokens::{IdentFragment, Spanned, ToTokens, Tokens};
pub use ty::{
    BareFnArg, TraitBound, TraitBoundModifier, Type, TypeArray, TypeBareFn, TypeGroup,
    TypeImplTrait, TypeMacro, TypeParamBound, TypeParen, TypePath, TypePtr, TypeReference,
    TypeSlice, TypeTraitObject, TypeTuple, parse_type, parse_type_bound, parse_type_bounds,
};

pub mod token {
    pub use crate::punctuated::{
        And, Colon, Comma, Dot, Eq, Gt, Lt, Not, Or, PathSep, Plus, Pound, RArrow, Semi, Star,
    };
}

#[doc(hidden)]
pub mod __private {
    pub use crate::tokens::ext;
    pub use crate::tokens::{
        FragmentAdapter, HasIterator, RepInterp, ThereIsNoIteratorInRepetition, make_ident,
        push_group, push_ident, push_lifetime, push_other, push_punct,
    };
}
