use nanocode_macro_support as syntax;
use nanocode_macro_support::Span;
use nanocode_macro_support::ToTokens;
use std::mem;
use syntax::{
    Data, DeriveInput, ExprPath, GenericArgument, GenericParam, Generics, Path, PathArguments,
    QSelf, ReturnType, Tokens, Type, TypeParamBound, TypePath, WherePredicate, parse_quote, respan,
};

pub fn replace_receiver(input: &mut DeriveInput) {
    let self_ty = {
        let ident = &input.ident;
        let ty_generics = input.generics.split_for_impl().1;
        parse_quote!(#ident #ty_generics)
    };
    let mut visitor = ReplaceReceiver(&self_ty);
    visitor.visit_generics_mut(&mut input.generics);
    visitor.visit_data_mut(&mut input.data);
}

struct ReplaceReceiver<'a>(&'a TypePath);

impl ReplaceReceiver<'_> {
    fn self_ty(&self, span: Span) -> TypePath {
        let tokens = self.0.to_token_stream();
        let respanned = respan(tokens, span);
        syntax::parse_tokens(respanned).unwrap()
    }

    fn self_to_qself(&self, qself: &mut Option<QSelf>, path: &mut Path) {
        if path.leading_colon.is_some() || path.segments[0].ident != "Self" {
            return;
        }

        if path.segments.len() == 1 {
            self.self_to_expr_path(path);
            return;
        }

        let span = path.segments[0].ident.span();
        *qself = Some(QSelf {
            lt_token: syntax::token::Lt { span },
            ty: Box::new(Type::Path(self.self_ty(span))),
            position: 0,
            as_token: None,
            gt_token: syntax::token::Gt { span },
        });

        path.leading_colon = Some(**path.segments.pairs().next().unwrap().punct().unwrap());

        let segments = mem::take(&mut path.segments);
        path.segments = segments.into_pairs().skip(1).collect();
    }

    fn self_to_expr_path(&self, path: &mut Path) {
        let self_ty = self.self_ty(path.segments[0].ident.span());
        let variant = mem::replace(path, self_ty.path);
        for segment in &mut path.segments {
            if let PathArguments::AngleBracketed(bracketed) = &mut segment.arguments
                && bracketed.colon2_token.is_none()
                && !bracketed.args.is_empty()
            {
                bracketed.colon2_token = Some(<syntax::token::PathSep>::default());
            }
        }
        if variant.segments.len() > 1 {
            path.segments
                .push_punct(<syntax::token::PathSep>::default());
            path.segments.extend(variant.segments.into_pairs().skip(1));
        }
    }
}

impl ReplaceReceiver<'_> {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        let span = if let Type::Path(node) = ty {
            if node.qself.is_none() && node.path.is_ident("Self") {
                node.path.segments[0].ident.span()
            } else {
                self.visit_type_path_mut(node);
                return;
            }
        } else {
            self.visit_type_mut_impl(ty);
            return;
        };
        *ty = Type::Path(self.self_ty(span));
    }

    fn visit_type_path_mut(&mut self, ty: &mut TypePath) {
        if ty.qself.is_none() {
            self.self_to_qself(&mut ty.qself, &mut ty.path);
        }
        self.visit_type_path_mut_impl(ty);
    }

    fn visit_expr_path_mut(&mut self, expr: &mut ExprPath) {
        if expr.qself.is_none() {
            self.self_to_qself(&mut expr.qself, &mut expr.path);
        }
        self.visit_expr_path_mut_impl(expr);
    }

    fn visit_type_mut_impl(&mut self, ty: &mut Type) {
        match ty {
            Type::Array(ty) => {
                self.visit_type_mut(&mut ty.elem);
                self.visit_expr_mut(&mut ty.len);
            }
            Type::BareFn(ty) => {
                for arg in &mut ty.inputs {
                    self.visit_type_mut(&mut arg.ty);
                }
                self.visit_return_type_mut(&mut ty.output);
            }
            Type::Group(ty) => self.visit_type_mut(&mut ty.elem),
            Type::ImplTrait(ty) => {
                for bound in &mut ty.bounds {
                    self.visit_type_param_bound_mut(bound);
                }
            }
            Type::Macro(_) => {}
            Type::Paren(ty) => self.visit_type_mut(&mut ty.elem),
            Type::Path(ty) => {
                if let Some(qself) = &mut ty.qself {
                    self.visit_type_mut(&mut qself.ty);
                }
                self.visit_path_mut(&mut ty.path);
            }
            Type::Ptr(ty) => self.visit_type_mut(&mut ty.elem),
            Type::Reference(ty) => self.visit_type_mut(&mut ty.elem),
            Type::Slice(ty) => self.visit_type_mut(&mut ty.elem),
            Type::TraitObject(ty) => {
                for bound in &mut ty.bounds {
                    self.visit_type_param_bound_mut(bound);
                }
            }
            Type::Tuple(ty) => {
                for elem in &mut ty.elems {
                    self.visit_type_mut(elem);
                }
            }

            Type::Infer(_) | Type::Never(_) | Type::Verbatim(_) => {}
        }
    }

    fn visit_type_path_mut_impl(&mut self, ty: &mut TypePath) {
        if let Some(qself) = &mut ty.qself {
            self.visit_type_mut(&mut qself.ty);
        }
        self.visit_path_mut(&mut ty.path);
    }

    fn visit_expr_path_mut_impl(&mut self, expr: &mut ExprPath) {
        if let Some(qself) = &mut expr.qself {
            self.visit_type_mut(&mut qself.ty);
        }
        self.visit_path_mut(&mut expr.path);
    }

    fn visit_path_mut(&mut self, path: &mut Path) {
        for segment in &mut path.segments {
            self.visit_path_arguments_mut(&mut segment.arguments);
        }
    }

    fn visit_path_arguments_mut(&mut self, arguments: &mut PathArguments) {
        match arguments {
            PathArguments::None => {}
            PathArguments::AngleBracketed(arguments) => {
                for arg in &mut arguments.args {
                    match arg {
                        GenericArgument::Type(arg) => self.visit_type_mut(arg),
                        GenericArgument::AssocType(arg) => self.visit_type_mut(&mut arg.ty),
                        GenericArgument::Lifetime(_)
                        | GenericArgument::Const(_)
                        | GenericArgument::AssocConst(_)
                        | GenericArgument::Constraint(_) => {}
                    }
                }
            }
            PathArguments::Parenthesized(arguments) => {
                for argument in &mut arguments.inputs {
                    self.visit_type_mut(argument);
                }
                self.visit_return_type_mut(&mut arguments.output);
            }
        }
    }

    fn visit_return_type_mut(&mut self, return_type: &mut ReturnType) {
        match return_type {
            ReturnType::Default => {}
            ReturnType::Type(_, output) => self.visit_type_mut(output),
        }
    }

    fn visit_type_param_bound_mut(&mut self, bound: &mut TypeParamBound) {
        match bound {
            TypeParamBound::Trait(bound) => self.visit_path_mut(&mut bound.path),
            TypeParamBound::Lifetime(_)
            | TypeParamBound::PreciseCapture(_)
            | TypeParamBound::Verbatim(_) => {}
        }
    }

    fn visit_generics_mut(&mut self, generics: &mut Generics) {
        for param in &mut generics.params {
            match param {
                GenericParam::Type(param) => {
                    for bound in &mut param.bounds {
                        self.visit_type_param_bound_mut(bound);
                    }
                }
                GenericParam::Lifetime(_) | GenericParam::Const(_) => {}
            }
        }
        if let Some(where_clause) = &mut generics.where_clause {
            for predicate in &mut where_clause.predicates {
                match predicate {
                    WherePredicate::Type(predicate) => {
                        self.visit_type_mut(&mut predicate.bounded_ty);
                        for bound in &mut predicate.bounds {
                            self.visit_type_param_bound_mut(bound);
                        }
                    }
                    WherePredicate::Lifetime(_) => {}
                }
            }
        }
    }

    fn visit_data_mut(&mut self, data: &mut Data) {
        match data {
            Data::Struct(data) => {
                for field in &mut data.fields {
                    self.visit_type_mut(&mut field.ty);
                }
            }
            Data::Enum(data) => {
                for variant in &mut data.variants {
                    for field in &mut variant.fields {
                        self.visit_type_mut(&mut field.ty);
                    }
                }
            }
            Data::Union(_) => {}
        }
    }

    fn visit_expr_mut(&mut self, expr: &mut Tokens) {
        let Ok(mut path) = syntax::parse_tokens::<ExprPath>(expr.clone()) else {
            return;
        };
        self.visit_expr_path_mut(&mut path);
        *expr = path.to_token_stream();
    }
}
