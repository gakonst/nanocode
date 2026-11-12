#![deny(rustdoc::broken_intra_doc_links)]

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    Error, FnArg, GenericArgument, ItemFn, LitStr, Pat, PathArguments, ReturnType, Token, Type,
    parse::Parser, punctuated::Punctuated,
};

struct ToolArgs {
    description: LitStr,
    name: Option<LitStr>,
    parallel: bool,
}

#[proc_macro_attribute]
pub fn tool(attributes: TokenStream, item: TokenStream) -> TokenStream {
    expand_tool(attributes.into(), item.into())
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

fn expand_tool(
    attributes: proc_macro2::TokenStream,
    item: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let arguments = parse_args(attributes)?;
    let function = syn::parse2::<ItemFn>(item)?;
    validate_signature(&function)?;
    let output = result_output(&function.sig.output)?;
    let original_ident = &function.sig.ident;
    let handler_ident = format_ident!("__nanocode_tool_{original_ident}");
    let input_ident = format_ident!("__NanocodeTool{}Input", pascal_case(original_ident));
    let visibility = &function.vis;
    let description = arguments.description;
    let parallel = arguments.parallel;
    let tool_name = arguments
        .name
        .unwrap_or_else(|| LitStr::new(&original_ident.to_string(), original_ident.span()));
    let (nanocode, nanocode_path) = nanocode_path();
    let serde_path = LitStr::new(
        &format!("{nanocode_path}::__private::serde"),
        Span::call_site(),
    );
    let schemars_path = LitStr::new(
        &format!("{nanocode_path}::__private::schemars"),
        Span::call_site(),
    );

    let (fields, calls) = function_arguments(&function)?;

    let mut hidden_function = function.clone();
    hidden_function.sig.ident = handler_ident.clone();
    hidden_function.vis = syn::Visibility::Inherited;
    let attrs = &function.attrs;

    Ok(quote! {
        #[allow(clippy::unused_async)]
        #hidden_function

        #(#attrs)*
        #[allow(non_camel_case_types)]
        #visibility struct #original_ident;

        #[derive(
            #nanocode::__private::serde::Deserialize,
            #nanocode::__private::schemars::JsonSchema
        )]
        #[serde(
            crate = #serde_path,
            deny_unknown_fields
        )]
        #[schemars(crate = #schemars_path)]
        struct #input_ident {
            #(#fields,)*
        }

        impl #nanocode::__private::Tool for #original_ident {
            fn definition(&self) -> #nanocode::__private::ToolDefinition {
                #nanocode::__private::ToolDefinition::function(
                    #tool_name,
                    #description,
                    #nanocode::__private::schema_for::<#input_ident>(),
                )
                .with_output_schema(#nanocode::__private::schema_for::<#output>())
            }

            fn supports_parallel_tool_calls(&self) -> bool {
                #parallel
            }

            fn execute<'life0, 'life1, 'async_trait>(
                &'life0 self,
                input: #nanocode::__private::ToolInput,
                _context: #nanocode::__private::ToolContext<'life1>,
            ) -> #nanocode::__private::ToolCallFuture<'async_trait, #nanocode::__private::ToolResult>
            where
                'life0: 'async_trait,
                'life1: 'async_trait,
                Self: 'async_trait,
            {
                ::std::boxed::Box::pin(async move {
                    let input = input.decode_json::<#input_ident>()?;
                    match #handler_ident(#(#calls),*).await {
                        Ok(output) => Ok(#nanocode::__private::ToolOutput::json(&output)),
                        Err(error) => Err(
                            ::std::io::Error::other(error.to_string()).into()
                        ),
                    }
                })
            }
        }
    })
}

fn validate_signature(function: &ItemFn) -> syn::Result<()> {
    if function.sig.asyncness.is_none() {
        return Err(Error::new_spanned(
            function.sig.fn_token,
            "#[tool] requires an async function",
        ));
    }
    if !function.sig.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &function.sig.generics,
            "#[tool] functions cannot be generic",
        ));
    }
    Ok(())
}

fn function_arguments(
    function: &ItemFn,
) -> syn::Result<(Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>)> {
    let mut fields = Vec::with_capacity(function.sig.inputs.len());
    let mut calls = Vec::with_capacity(function.sig.inputs.len());
    for argument in &function.sig.inputs {
        let FnArg::Typed(argument) = argument else {
            return Err(Error::new_spanned(
                argument,
                "#[tool] does not support receivers",
            ));
        };
        let Pat::Ident(pattern) = argument.pat.as_ref() else {
            return Err(Error::new_spanned(
                &argument.pat,
                "#[tool] arguments must use simple identifier patterns",
            ));
        };
        let ident = &pattern.ident;
        let ty = &argument.ty;
        fields.push(quote!(#ident: #ty));
        calls.push(quote!(input.#ident));
    }
    Ok((fields, calls))
}

fn parse_args(attributes: proc_macro2::TokenStream) -> syn::Result<ToolArgs> {
    let entries =
        Punctuated::<syn::MetaNameValue, Token![,]>::parse_terminated.parse2(attributes)?;
    let mut description = None;
    let mut name = None;
    let mut parallel = None;
    for entry in entries {
        let Some(ident) = entry.path.get_ident() else {
            return Err(Error::new_spanned(
                entry.path,
                "unsupported #[tool] argument",
            ));
        };
        match ident.to_string().as_str() {
            "description" => {
                if description.is_some() {
                    return Err(Error::new_spanned(ident, "duplicate #[tool] argument"));
                }
                description = Some(parse_string_literal(entry.value)?);
            }
            "name" => {
                if name.is_some() {
                    return Err(Error::new_spanned(ident, "duplicate #[tool] argument"));
                }
                name = Some(parse_string_literal(entry.value)?);
            }
            "parallel" => {
                if parallel.is_some() {
                    return Err(Error::new_spanned(ident, "duplicate #[tool] argument"));
                }
                parallel = Some(parse_bool_literal(entry.value)?);
            }
            _ => return Err(Error::new_spanned(ident, "unsupported #[tool] argument")),
        }
    }
    let Some(description) = description else {
        return Err(Error::new(
            Span::call_site(),
            "#[tool] requires description = \"...\"",
        ));
    };
    Ok(ToolArgs {
        description,
        name,
        parallel: parallel.unwrap_or(false),
    })
}

fn parse_string_literal(expression: syn::Expr) -> syn::Result<LitStr> {
    let syn::Expr::Lit(expression) = expression else {
        return Err(Error::new_spanned(expression, "expected a string literal"));
    };
    let syn::Lit::Str(value) = expression.lit else {
        return Err(Error::new_spanned(
            expression.lit,
            "expected a string literal",
        ));
    };
    Ok(value)
}

fn parse_bool_literal(expression: syn::Expr) -> syn::Result<bool> {
    let syn::Expr::Lit(expression) = expression else {
        return Err(Error::new_spanned(expression, "expected a boolean literal"));
    };
    let syn::Lit::Bool(value) = expression.lit else {
        return Err(Error::new_spanned(
            expression.lit,
            "expected a boolean literal",
        ));
    };
    Ok(value.value)
}

fn result_output(output: &ReturnType) -> syn::Result<&Type> {
    let ReturnType::Type(_, ty) = output else {
        return Err(Error::new_spanned(
            output,
            "#[tool] functions must return Result<T, E>",
        ));
    };
    let Type::Path(path) = ty.as_ref() else {
        return Err(Error::new_spanned(
            ty,
            "#[tool] functions must return Result<T, E>",
        ));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new_spanned(ty, "expected Result<T, E>"));
    };
    if segment.ident != "Result" {
        return Err(Error::new_spanned(ty, "expected Result<T, E>"));
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(Error::new_spanned(ty, "expected Result<T, E>"));
    };
    arguments
        .args
        .iter()
        .find_map(|argument| match argument {
            GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .ok_or_else(|| Error::new_spanned(ty, "expected Result<T, E>"))
}

fn nanocode_path() -> (proc_macro2::TokenStream, String) {
    for package in [
        "nanocode-tools",
        "nanocode",
        "nanocode-code-mode",
        "nanocode-tool-runtime",
        "nanocode-backend-codex",
        "nanocode-backend-claude",
    ] {
        if let Ok(found) = crate_name(package) {
            let name = match found {
                FoundCrate::Itself => package.replace('-', "_"),
                FoundCrate::Name(name) => name,
            };
            let ident = syn::Ident::new(&name, Span::call_site());
            return (quote!(::#ident), format!("::{name}"));
        }
    }
    (quote!(::nanocode_tools), "::nanocode_tools".to_owned())
}

fn pascal_case(ident: &syn::Ident) -> String {
    ident
        .to_string()
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect()
}
