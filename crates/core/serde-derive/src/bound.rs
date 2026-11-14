use crate::internals::ast::{Container, Data};
use crate::internals::{attr, ungroup};
use nanocode_macro_support as syntax;
use nanocode_macro_support::Span;
use std::collections::HashSet;
use syntax::{Pair, Punctuated};

pub fn without_defaults(generics: &syntax::Generics) -> syntax::Generics {
    syntax::Generics {
        params: generics
            .params
            .iter()
            .map(|param| match param {
                syntax::GenericParam::Type(param) => {
                    syntax::GenericParam::Type(syntax::TypeParam {
                        eq_token: None,
                        default: None,
                        ..param.clone()
                    })
                }
                _ => param.clone(),
            })
            .collect(),
        ..generics.clone()
    }
}

pub fn with_where_predicates(
    generics: &syntax::Generics,
    predicates: &[syntax::WherePredicate],
) -> syntax::Generics {
    let mut generics = generics.clone();
    generics
        .make_where_clause()
        .predicates
        .extend(predicates.iter().cloned());
    generics
}

pub fn with_where_predicates_from_fields(
    cont: &Container,
    generics: &syntax::Generics,
    from_field: fn(&attr::Field) -> Option<&[syntax::WherePredicate]>,
) -> syntax::Generics {
    let predicates = cont
        .data
        .all_fields()
        .filter_map(|field| from_field(&field.attrs))
        .flat_map(<[syntax::WherePredicate]>::to_vec);

    let mut generics = generics.clone();
    generics.make_where_clause().predicates.extend(predicates);
    generics
}

pub fn with_where_predicates_from_variants(
    cont: &Container,
    generics: &syntax::Generics,
    from_variant: fn(&attr::Variant) -> Option<&[syntax::WherePredicate]>,
) -> syntax::Generics {
    let variants = match &cont.data {
        Data::Enum(variants) => variants,
        Data::Struct(_, _) => {
            return generics.clone();
        }
    };

    let predicates = variants
        .iter()
        .filter_map(|variant| from_variant(&variant.attrs))
        .flat_map(<[syntax::WherePredicate]>::to_vec);

    let mut generics = generics.clone();
    generics.make_where_clause().predicates.extend(predicates);
    generics
}

pub fn with_bound(
    cont: &Container,
    generics: &syntax::Generics,
    filter: fn(&attr::Field, Option<&attr::Variant>) -> bool,
    bound: &syntax::Path,
) -> syntax::Generics {
    struct FindTyParams<'ast> {
        all_type_params: HashSet<syntax::Ident>,

        relevant_type_params: HashSet<syntax::Ident>,

        associated_type_usage: Vec<&'ast syntax::TypePath>,
    }

    impl<'ast> FindTyParams<'ast> {
        fn visit_field(&mut self, field: &'ast syntax::Field) {
            if let syntax::Type::Path(ty) = ungroup(&field.ty)
                && let Some(Pair::Punctuated(t, _)) = ty.path.segments.pairs().next()
                && self.all_type_params.contains(&t.ident)
            {
                self.associated_type_usage.push(ty);
            }
            self.visit_type(&field.ty);
        }

        fn visit_path(&mut self, path: &'ast syntax::Path) {
            if let Some(seg) = path.segments.last()
                && seg.ident == "PhantomData"
            {
                return;
            }
            if path.leading_colon.is_none() && path.segments.len() == 1 {
                let id = &path.segments[0].ident;
                if self.all_type_params.contains(id) {
                    self.relevant_type_params.insert(id.clone());
                }
            }
            for segment in &path.segments {
                self.visit_path_segment(segment);
            }
        }

        fn visit_type(&mut self, ty: &'ast syntax::Type) {
            match ty {
                syntax::Type::Array(ty) => self.visit_type(&ty.elem),
                syntax::Type::BareFn(ty) => {
                    for arg in &ty.inputs {
                        self.visit_type(&arg.ty);
                    }
                    self.visit_return_type(&ty.output);
                }
                syntax::Type::Group(ty) => self.visit_type(&ty.elem),
                syntax::Type::ImplTrait(ty) => {
                    for bound in &ty.bounds {
                        self.visit_type_param_bound(bound);
                    }
                }
                syntax::Type::Macro(ty) => self.visit_macro(ty),
                syntax::Type::Paren(ty) => self.visit_type(&ty.elem),
                syntax::Type::Path(ty) => {
                    if let Some(qself) = &ty.qself {
                        self.visit_type(&qself.ty);
                    }
                    self.visit_path(&ty.path);
                }
                syntax::Type::Ptr(ty) => self.visit_type(&ty.elem),
                syntax::Type::Reference(ty) => self.visit_type(&ty.elem),
                syntax::Type::Slice(ty) => self.visit_type(&ty.elem),
                syntax::Type::TraitObject(ty) => {
                    for bound in &ty.bounds {
                        self.visit_type_param_bound(bound);
                    }
                }
                syntax::Type::Tuple(ty) => {
                    for elem in &ty.elems {
                        self.visit_type(elem);
                    }
                }

                syntax::Type::Infer(_) | syntax::Type::Never(_) | syntax::Type::Verbatim(_) => {}
            }
        }

        fn visit_path_segment(&mut self, segment: &'ast syntax::PathSegment) {
            self.visit_path_arguments(&segment.arguments);
        }

        fn visit_path_arguments(&mut self, arguments: &'ast syntax::PathArguments) {
            match arguments {
                syntax::PathArguments::None => {}
                syntax::PathArguments::AngleBracketed(arguments) => {
                    for arg in &arguments.args {
                        match arg {
                            syntax::GenericArgument::Type(arg) => self.visit_type(arg),
                            syntax::GenericArgument::AssocType(arg) => self.visit_type(&arg.ty),
                            syntax::GenericArgument::Lifetime(_)
                            | syntax::GenericArgument::Const(_)
                            | syntax::GenericArgument::AssocConst(_)
                            | syntax::GenericArgument::Constraint(_) => {}
                        }
                    }
                }
                syntax::PathArguments::Parenthesized(arguments) => {
                    for argument in &arguments.inputs {
                        self.visit_type(argument);
                    }
                    self.visit_return_type(&arguments.output);
                }
            }
        }

        fn visit_return_type(&mut self, return_type: &'ast syntax::ReturnType) {
            match return_type {
                syntax::ReturnType::Default => {}
                syntax::ReturnType::Type(_, output) => self.visit_type(output),
            }
        }

        fn visit_type_param_bound(&mut self, bound: &'ast syntax::TypeParamBound) {
            match bound {
                syntax::TypeParamBound::Trait(bound) => self.visit_path(&bound.path),
                syntax::TypeParamBound::Lifetime(_)
                | syntax::TypeParamBound::PreciseCapture(_)
                | syntax::TypeParamBound::Verbatim(_) => {}
            }
        }

        const fn visit_macro(&mut self, _mac: &'ast syntax::TypeMacro) {}
    }

    let all_type_params = generics
        .type_params()
        .map(|param| param.ident.clone())
        .collect();

    let mut visitor = FindTyParams {
        all_type_params,
        relevant_type_params: HashSet::new(),
        associated_type_usage: Vec::new(),
    };
    match &cont.data {
        Data::Enum(variants) => {
            for variant in variants {
                let relevant_fields = variant
                    .fields
                    .iter()
                    .filter(|field| filter(&field.attrs, Some(&variant.attrs)));
                for field in relevant_fields {
                    visitor.visit_field(field.original);
                }
            }
        }
        Data::Struct(_, fields) => {
            for field in fields.iter().filter(|field| filter(&field.attrs, None)) {
                visitor.visit_field(field.original);
            }
        }
    }

    let relevant_type_params = visitor.relevant_type_params;
    let associated_type_usage = visitor.associated_type_usage;
    let new_predicates = generics
        .type_params()
        .map(|param| param.ident.clone())
        .filter(|id| relevant_type_params.contains(id))
        .map(|id| syntax::TypePath {
            qself: None,
            path: id.into(),
        })
        .chain(associated_type_usage.into_iter().cloned())
        .map(|bounded_ty| {
            syntax::WherePredicate::Type(syntax::PredicateType {
                lifetimes: None,

                bounded_ty: syntax::Type::Path(bounded_ty),
                colon_token: <syntax::token::Colon>::default(),

                bounds: vec![syntax::TypeParamBound::Trait(syntax::TraitBound {
                    paren_span: None,
                    modifier: syntax::TraitBoundModifier::None,
                    lifetimes: None,
                    path: bound.clone(),
                })]
                .into_iter()
                .collect(),
            })
        });

    let mut generics = generics.clone();
    generics
        .make_where_clause()
        .predicates
        .extend(new_predicates);
    generics
}

pub fn with_self_bound(
    cont: &Container,
    generics: &syntax::Generics,
    bound: &syntax::Path,
) -> syntax::Generics {
    let mut generics = generics.clone();
    generics
        .make_where_clause()
        .predicates
        .push(syntax::WherePredicate::Type(syntax::PredicateType {
            lifetimes: None,

            bounded_ty: type_of_item(cont),
            colon_token: <syntax::token::Colon>::default(),

            bounds: vec![syntax::TypeParamBound::Trait(syntax::TraitBound {
                paren_span: None,
                modifier: syntax::TraitBoundModifier::None,
                lifetimes: None,
                path: bound.clone(),
            })]
            .into_iter()
            .collect(),
        }));
    generics
}

pub fn with_lifetime_bound(generics: &syntax::Generics, lifetime: &str) -> syntax::Generics {
    let bound = syntax::Lifetime::new(lifetime, Span::call_site());
    let def = syntax::LifetimeParam {
        attrs: Vec::new(),
        lifetime: bound.clone(),
        colon_token: None,
        bounds: Punctuated::new(),
    };

    let params = Some(syntax::GenericParam::Lifetime(def))
        .into_iter()
        .chain(generics.params.iter().cloned().map(|mut param| {
            match &mut param {
                syntax::GenericParam::Lifetime(param) => {
                    param.bounds.push(bound.clone());
                }
                syntax::GenericParam::Type(param) => {
                    param
                        .bounds
                        .push(syntax::TypeParamBound::Lifetime(bound.clone()));
                }
                syntax::GenericParam::Const(_) => {}
            }
            param
        }))
        .collect();

    syntax::Generics {
        params,
        ..generics.clone()
    }
}

fn type_of_item(cont: &Container) -> syntax::Type {
    syntax::Type::Path(syntax::TypePath {
        qself: None,
        path: syntax::Path {
            leading_colon: None,
            segments: vec![syntax::PathSegment {
                ident: cont.ident.clone(),
                arguments: syntax::PathArguments::AngleBracketed(
                    syntax::AngleBracketedGenericArguments {
                        colon2_token: None,
                        lt_token: <syntax::token::Lt>::default(),
                        args: cont
                            .generics
                            .params
                            .iter()
                            .map(|param| match param {
                                syntax::GenericParam::Type(param) => syntax::GenericArgument::Type(
                                    syntax::Type::Path(syntax::TypePath {
                                        qself: None,
                                        path: param.ident.clone().into(),
                                    }),
                                ),
                                syntax::GenericParam::Lifetime(param) => {
                                    syntax::GenericArgument::Lifetime(param.lifetime.clone())
                                }
                                syntax::GenericParam::Const(_) => {
                                    panic!("Serde does not support const generics yet");
                                }
                            })
                            .collect(),
                        gt_token: <syntax::token::Gt>::default(),
                    },
                ),
            }]
            .into_iter()
            .collect(),
        },
    })
}
