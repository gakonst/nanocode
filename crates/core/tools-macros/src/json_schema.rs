use nanocode_macro_support::{
    Attribute, Data, DataEnum, DeriveInput, Error, ExprPath, Fields, Ident, Lit, LitStr, Member,
    Meta, Parse, Span, Tokens, Type, TypeParamBound, parse_terminated, parse_tokens, quote,
    token::Comma,
};
use std::collections::BTreeSet;

const UNSUPPORTED_SERDE_ATTRIBUTES: [&str; 3] = ["remote", "with", "rename_all_fields"];

#[derive(Clone, Copy)]
enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    fn parse(text: &LitStr) -> nanocode_macro_support::Result<Self> {
        match text.value().as_str() {
            "lowercase" => Ok(Self::Lower),
            "UPPERCASE" => Ok(Self::Upper),
            "PascalCase" => Ok(Self::Pascal),
            "camelCase" => Ok(Self::Camel),
            "snake_case" => Ok(Self::Snake),
            "SCREAMING_SNAKE_CASE" => Ok(Self::ScreamingSnake),
            "kebab-case" => Ok(Self::Kebab),
            "SCREAMING-KEBAB-CASE" => Ok(Self::ScreamingKebab),
            _ => Err(Error::new_spanned(text, "unknown rename rule")),
        }
    }

    fn apply_to_variant(self, variant: &str) -> String {
        match self {
            Self::Pascal => variant.to_owned(),
            Self::Lower => variant.to_ascii_lowercase(),
            Self::Upper => variant.to_ascii_uppercase(),
            Self::Camel => lowercase_first(variant),
            Self::Snake => {
                let mut snake = String::new();
                for (index, character) in variant.char_indices() {
                    if index > 0 && character.is_uppercase() {
                        snake.push('_');
                    }
                    snake.push(character.to_ascii_lowercase());
                }
                snake
            }
            Self::ScreamingSnake => Self::Snake.apply_to_variant(variant).to_ascii_uppercase(),
            Self::Kebab => Self::Snake.apply_to_variant(variant).replace('_', "-"),
            Self::ScreamingKebab => Self::ScreamingSnake
                .apply_to_variant(variant)
                .replace('_', "-"),
        }
    }

    fn apply_to_field(self, field: &str) -> String {
        match self {
            Self::Lower | Self::Snake => field.to_owned(),
            Self::Upper | Self::ScreamingSnake => field.to_ascii_uppercase(),
            Self::Pascal => {
                let mut pascal = String::new();
                let mut capitalize = true;
                for character in field.chars() {
                    if character == '_' {
                        capitalize = true;
                    } else if capitalize {
                        pascal.push(character.to_ascii_uppercase());
                        capitalize = false;
                    } else {
                        pascal.push(character);
                    }
                }
                pascal
            }
            Self::Camel => lowercase_first(&Self::Pascal.apply_to_field(field)),
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => field.to_ascii_uppercase().replace('_', "-"),
        }
    }
}

fn lowercase_first(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map_or_else(String::new, |first| {
        let mut lowered = first.to_ascii_lowercase().to_string();
        lowered.push_str(characters.as_str());
        lowered
    })
}

enum DefaultValue {
    Trait,
    Function(ExprPath),
}

enum Tagging {
    External,
    Internal { tag: String },
    Adjacent { tag: String, content: String },
    Untagged,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Unit,
    Newtype,
    Tuple,
    Struct,
}

#[derive(Default)]
struct Documentation {
    title: Option<String>,
    description: Option<String>,
    deprecated: bool,
}

#[derive(Default)]
struct ContainerAttributes {
    rename: Option<String>,
    rename_all: Option<RenameRule>,
    deny_unknown_fields: bool,
    tag: Option<String>,
    content: Option<String>,
    untagged: bool,
    default: Option<DefaultValue>,
    transparent: bool,
}

#[derive(Default)]
struct FieldAttributes {
    rename: Option<String>,
    default: Option<DefaultValue>,
    skip_serializing: bool,
    skip_deserializing: bool,
    skip_serializing_if: Option<ExprPath>,
    flatten: bool,
    serialize_with: bool,
}

#[derive(Default)]
struct VariantAttributes {
    rename: Option<String>,
    rename_all: Option<RenameRule>,
    skip_deserializing: bool,
}

struct FieldModel<'a> {
    member: Member,
    name: String,
    ty: &'a Type,
    attributes: FieldAttributes,
    documentation: Documentation,
}

struct VariantModel<'a> {
    name: String,
    style: Style,
    fields: Vec<FieldModel<'a>>,
    documentation: Documentation,
}

fn unraw(ident: &Ident) -> String {
    ident.to_string().trim_start_matches("r#").to_owned()
}

fn serde_metas(attributes: &[Attribute]) -> nanocode_macro_support::Result<Vec<Meta>> {
    let mut metas = Vec::new();
    for attribute in attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("serde"))
    {
        let list = attribute.meta.require_list()?;
        metas.extend(
            list.parse_args_with(|input| parse_terminated::<Meta, Comma>(input, Meta::parse))?,
        );
    }
    Ok(metas)
}

fn meta_name(meta: &Meta) -> String {
    meta.path()
        .get_ident()
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn string_value(meta: &Meta) -> nanocode_macro_support::Result<LitStr> {
    let Meta::NameValue(pair) = meta else {
        return Err(Error::new_spanned(
            meta,
            "JsonSchema supports only the name = \"...\" form of this serde attribute",
        ));
    };
    let Some(Lit::Str(text)) = pair.value_lit() else {
        return Err(Error::new_spanned(&pair.value, "expected a string literal"));
    };
    Ok(text)
}

fn flag(meta: &Meta) -> nanocode_macro_support::Result<bool> {
    match meta {
        Meta::Path(_) => Ok(true),
        _ => Err(Error::new_spanned(meta, "expected a bare serde flag")),
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, meta: &Meta) -> nanocode_macro_support::Result<()> {
    if slot.is_some() {
        return Err(Error::new_spanned(meta, "duplicate serde attribute"));
    }
    *slot = Some(value);
    Ok(())
}

fn default_value(meta: &Meta) -> nanocode_macro_support::Result<DefaultValue> {
    match meta {
        Meta::Path(_) => Ok(DefaultValue::Trait),
        _ => Ok(DefaultValue::Function(string_value(meta)?.parse()?)),
    }
}

fn rejected(meta: &Meta) -> nanocode_macro_support::Result<()> {
    if UNSUPPORTED_SERDE_ATTRIBUTES.contains(&meta_name(meta).as_str()) {
        return Err(Error::new_spanned(
            meta,
            "JsonSchema does not support this serde attribute",
        ));
    }
    Ok(())
}

impl ContainerAttributes {
    fn parse(attributes: &[Attribute]) -> nanocode_macro_support::Result<Self> {
        let mut parsed = Self::default();
        for meta in serde_metas(attributes)? {
            match meta_name(&meta).as_str() {
                "rename" => set_once(&mut parsed.rename, string_value(&meta)?.value(), &meta)?,
                "rename_all" => set_once(
                    &mut parsed.rename_all,
                    RenameRule::parse(&string_value(&meta)?)?,
                    &meta,
                )?,
                "deny_unknown_fields" => parsed.deny_unknown_fields = flag(&meta)?,
                "tag" => set_once(&mut parsed.tag, string_value(&meta)?.value(), &meta)?,
                "content" => set_once(&mut parsed.content, string_value(&meta)?.value(), &meta)?,
                "untagged" => parsed.untagged = flag(&meta)?,
                "transparent" => parsed.transparent = flag(&meta)?,
                "default" => set_once(&mut parsed.default, default_value(&meta)?, &meta)?,
                _ => rejected(&meta)?,
            }
        }
        Ok(parsed)
    }

    fn tagging(&self) -> nanocode_macro_support::Result<Tagging> {
        match (self.untagged, &self.tag, &self.content) {
            (false, None, None) => Ok(Tagging::External),
            (true, None, None) => Ok(Tagging::Untagged),
            (false, Some(tag), None) => Ok(Tagging::Internal { tag: tag.clone() }),
            (false, Some(tag), Some(content)) => Ok(Tagging::Adjacent {
                tag: tag.clone(),
                content: content.clone(),
            }),
            _ => Err(Error::new(
                Span::call_site(),
                "conflicting serde enum representation attributes",
            )),
        }
    }
}

impl FieldAttributes {
    fn parse(
        attributes: &[Attribute],
        container_has_default: bool,
    ) -> nanocode_macro_support::Result<Self> {
        let mut parsed = Self::default();
        for meta in serde_metas(attributes)? {
            match meta_name(&meta).as_str() {
                "rename" => set_once(&mut parsed.rename, string_value(&meta)?.value(), &meta)?,
                "default" => set_once(&mut parsed.default, default_value(&meta)?, &meta)?,
                "skip" => {
                    parsed.skip_serializing = flag(&meta)?;
                    parsed.skip_deserializing = true;
                }
                "skip_serializing" => parsed.skip_serializing = flag(&meta)?,
                "skip_deserializing" => parsed.skip_deserializing = flag(&meta)?,
                "skip_serializing_if" => set_once(
                    &mut parsed.skip_serializing_if,
                    string_value(&meta)?.parse()?,
                    &meta,
                )?,
                "flatten" => parsed.flatten = flag(&meta)?,
                "serialize_with" => parsed.serialize_with = true,
                _ => rejected(&meta)?,
            }
        }
        if !container_has_default && parsed.skip_deserializing && parsed.default.is_none() {
            parsed.default = Some(DefaultValue::Trait);
        }
        Ok(parsed)
    }
}

impl VariantAttributes {
    fn parse(attributes: &[Attribute]) -> nanocode_macro_support::Result<Self> {
        let mut parsed = Self::default();
        for meta in serde_metas(attributes)? {
            match meta_name(&meta).as_str() {
                "rename" => set_once(&mut parsed.rename, string_value(&meta)?.value(), &meta)?,
                "rename_all" => set_once(
                    &mut parsed.rename_all,
                    RenameRule::parse(&string_value(&meta)?)?,
                    &meta,
                )?,
                "skip" | "skip_deserializing" => parsed.skip_deserializing = flag(&meta)?,
                "untagged" => {
                    return Err(Error::new_spanned(
                        &meta,
                        "JsonSchema does not support untagged variants",
                    ));
                }
                _ => rejected(&meta)?,
            }
        }
        Ok(parsed)
    }
}

impl Documentation {
    fn parse(attributes: &[Attribute]) -> Self {
        let deprecated = attributes
            .iter()
            .any(|attribute| attribute.path().is_ident("deprecated"));
        let Some(documentation) = documentation(attributes) else {
            return Self {
                deprecated,
                ..Self::default()
            };
        };
        let (title, description) = if documentation.starts_with('#') {
            let (first, rest) = documentation
                .split_once('\n')
                .map_or((documentation.as_str(), None), |(first, rest)| {
                    (first, Some(rest))
                });
            let title = first.trim_start_matches('#').trim().to_owned();
            (non_empty(title), rest.and_then(merge_description_lines))
        } else {
            (None, merge_description_lines(&documentation))
        };
        Self {
            title,
            description,
            deprecated,
        }
    }

    const fn is_plain(&self) -> bool {
        self.title.is_none() && self.description.is_none() && !self.deprecated
    }

    fn apply(&self, private: &Tokens, schema: Tokens) -> Tokens {
        let mut schema = schema;
        if let Some(title) = &self.title {
            schema = quote!(#private::with_title(#schema, #title));
        }
        if let Some(description) = &self.description {
            schema = quote!(#private::with_description(#schema, #description));
        }
        if self.deprecated {
            schema = quote!(#private::deprecated(#schema));
        }
        schema
    }
}

fn documentation(attributes: &[Attribute]) -> Option<String> {
    let texts: Vec<String> = attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .filter_map(|attribute| {
            let pair = attribute.meta.require_name_value().ok()?;
            match pair.value_lit() {
                Some(Lit::Str(text)) => Some(text.value()),
                _ => None,
            }
        })
        .collect();
    let mut lines: Vec<&str> = texts
        .iter()
        .flat_map(|text| text.split('\n'))
        .map(str::trim)
        .skip_while(|line| line.is_empty())
        .collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    if lines.iter().all(|line| line.starts_with('*')) {
        for line in &mut lines {
            *line = line[1..].trim();
        }
        let leading_empty = lines.iter().take_while(|line| line.is_empty()).count();
        lines.drain(..leading_empty);
    }
    non_empty(lines.join("\n"))
}

fn merge_description_lines(documentation: &str) -> Option<String> {
    let paragraphs: Vec<String> = documentation
        .trim()
        .split("\n\n")
        .filter_map(|paragraph| non_empty(paragraph.trim().replace('\n', " ")))
        .collect();
    non_empty(paragraphs.join("\n\n"))
}

fn non_empty(text: String) -> Option<String> {
    if text.is_empty() { None } else { Some(text) }
}

fn style(fields: &Fields) -> Style {
    match fields {
        Fields::Named(_) => Style::Struct,
        Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => Style::Newtype,
        Fields::Unnamed(_) => Style::Tuple,
        Fields::Unit => Style::Unit,
    }
}

fn field_models(
    fields: &Fields,
    rename_all: Option<RenameRule>,
    container_has_default: bool,
) -> nanocode_macro_support::Result<Vec<FieldModel<'_>>> {
    let mut models = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        let attributes = FieldAttributes::parse(&field.attrs, container_has_default)?;
        let (member, source_name) = match &field.ident {
            Some(ident) => (Member::Named(ident.clone()), unraw(ident)),
            None => (Member::Unnamed(index.into()), index.to_string()),
        };
        let name = attributes.rename.clone().unwrap_or_else(|| {
            rename_all.map_or_else(
                || source_name.clone(),
                |rule| rule.apply_to_field(&source_name),
            )
        });
        models.push(FieldModel {
            member,
            name,
            ty: &field.ty,
            attributes,
            documentation: Documentation::parse(&field.attrs),
        });
    }
    Ok(models)
}

struct Generator<'a> {
    private: Tokens,
    container: &'a ContainerAttributes,
}

impl Generator<'_> {
    fn structure(&self, fields: &Fields) -> nanocode_macro_support::Result<Tokens> {
        let has_default = self.container.default.is_some();
        let models = field_models(fields, self.container.rename_all, has_default)?;
        match style(fields) {
            Style::Unit => Ok(quote!(generator.subschema_for::<()>())),
            Style::Newtype => {
                let ty = models[0].ty;
                Ok(quote!(generator.subschema_for::<#ty>()))
            }
            Style::Tuple => Ok(self.tuple(&models)),
            Style::Struct => self.object(
                &models,
                self.container.default.as_ref(),
                self.container.deny_unknown_fields,
            ),
        }
    }

    fn tuple(&self, fields: &[FieldModel<'_>]) -> Tokens {
        let private = &self.private;
        let types = fields
            .iter()
            .filter(|field| !field.attributes.skip_deserializing)
            .map(|field| field.ty);
        quote!(#private::tuple(::std::vec![#(generator.subschema_for::<#types>()),*]))
    }

    fn object(
        &self,
        fields: &[FieldModel<'_>],
        container_default: Option<&DefaultValue>,
        deny_unknown_fields: bool,
    ) -> nanocode_macro_support::Result<Tokens> {
        let private = &self.private;
        let (flattened, properties): (Vec<_>, Vec<_>) = fields
            .iter()
            .filter(|field| {
                !field.attributes.skip_deserializing || !field.attributes.skip_serializing
            })
            .partition(|field| field.attributes.flatten);
        let container_default_statement = match container_default {
            None => None,
            Some(DefaultValue::Trait) => Some(quote!(let container_default = Self::default();)),
            Some(DefaultValue::Function(path)) => Some(quote!(let container_default = #path();)),
        };
        let mut statements = Vec::with_capacity(properties.len());
        for field in properties {
            let ty = field.ty;
            let name = &field.name;
            let default = field_default(field, container_default.is_some())?;
            let has_default = default.is_some();
            let mut schema = field
                .documentation
                .apply(private, quote!(generator.subschema_for::<#ty>()));
            if field.attributes.skip_deserializing {
                schema = quote!(#private::read_only(#schema));
            }
            if field.attributes.skip_serializing {
                schema = quote!(#private::write_only(#schema));
            }
            if let Some(default) = default {
                schema = quote! {
                    #private::with_default(
                        #schema,
                        (#default).map(|default| #private::default_value(&default)),
                    )
                };
            }
            statements.push(quote! {
                #private::insert_property::<#ty>(&mut schema_object, #name, #has_default, #schema);
            });
        }
        let flattens = flattened.iter().map(|field| {
            let ty = field.ty;
            quote!(.flatten(#private::flattened::<#ty>(generator)))
        });
        Ok(quote! {{
            #container_default_statement
            let mut schema_object = #private::struct_object(#deny_unknown_fields);
            #(#statements)*
            #private::Schema::Object(schema_object) #(#flattens)*
        }})
    }

    fn enumeration(&self, data: &DataEnum) -> nanocode_macro_support::Result<Tokens> {
        let tagging = self.container.tagging()?;
        let mut variants = Vec::with_capacity(data.variants.len());
        for variant in &data.variants {
            let attributes = VariantAttributes::parse(&variant.attrs)?;
            if attributes.skip_deserializing {
                continue;
            }
            let source_name = unraw(&variant.ident);
            let name = attributes.rename.clone().unwrap_or_else(|| {
                self.container.rename_all.map_or_else(
                    || source_name.clone(),
                    |rule| rule.apply_to_variant(&source_name),
                )
            });
            variants.push(VariantModel {
                name,
                style: style(&variant.fields),
                fields: field_models(&variant.fields, attributes.rename_all, false)?,
                documentation: Documentation::parse(&variant.attrs),
            });
        }
        let names: BTreeSet<&str> = variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect();
        let unique = names.len() == variants.len();
        let private = &self.private;
        let deny = self.container.deny_unknown_fields;
        let schemas = match tagging {
            Tagging::External => return self.externally_tagged(&variants, unique),
            Tagging::Untagged => {
                let mut schemas = Vec::with_capacity(variants.len());
                for variant in &variants {
                    let schema = self.untagged_variant(variant, deny)?;
                    schemas.push(variant.documentation.apply(private, schema));
                }
                return Ok(quote!(#private::variants(false, ::std::vec![#(#schemas),*])));
            }
            Tagging::Internal { tag } => {
                let mut schemas = Vec::with_capacity(variants.len());
                for variant in &variants {
                    let name = &variant.name;
                    let mut schema = variant.documentation.apply(
                        private,
                        quote!(#private::internally_tagged_variant(#tag, #name, #deny)),
                    );
                    if let Some(content) = self.flattened_variant(variant, deny)? {
                        schema = quote!(#schema.flatten(#content));
                    }
                    schemas.push(schema);
                }
                schemas
            }
            Tagging::Adjacent { tag, content } => {
                let mut schemas = Vec::with_capacity(variants.len());
                for variant in &variants {
                    let name = &variant.name;
                    let content_schema = if variant.style == Style::Unit {
                        quote!(::core::option::Option::None)
                    } else {
                        let schema = self.untagged_variant(variant, deny)?;
                        quote!(::core::option::Option::Some(#schema))
                    };
                    schemas.push(variant.documentation.apply(
                        private,
                        quote! {
                            #private::adjacently_tagged_variant(#tag, #content, #name, #content_schema, #deny)
                        },
                    ));
                }
                schemas
            }
        };
        Ok(quote!(#private::variants(#unique, ::std::vec![#(#schemas),*])))
    }

    fn externally_tagged(
        &self,
        variants: &[VariantModel<'_>],
        unique: bool,
    ) -> nanocode_macro_support::Result<Tokens> {
        let private = &self.private;
        let (unit, complex): (Vec<_>, Vec<_>) = variants
            .iter()
            .partition(|variant| variant.style == Style::Unit && variant.documentation.is_plain());
        let unit_names = unit.iter().map(|variant| &variant.name);
        let unit_schema = quote!(#private::string_enum(&[#(#unit_names),*]));
        if complex.is_empty() {
            return Ok(unit_schema);
        }
        let mut schemas = Vec::with_capacity(complex.len() + 1);
        if !unit.is_empty() {
            schemas.push(unit_schema);
        }
        for variant in complex {
            let name = &variant.name;
            let schema = if variant.style == Style::Unit {
                quote!(#private::string_enum(&[#name]))
            } else {
                let content = self.untagged_variant(variant, self.container.deny_unknown_fields)?;
                quote!(#private::externally_tagged_variant(#name, #content))
            };
            schemas.push(variant.documentation.apply(private, schema));
        }
        Ok(quote!(#private::variants(#unique, ::std::vec![#(#schemas),*])))
    }

    fn untagged_variant(
        &self,
        variant: &VariantModel<'_>,
        deny: bool,
    ) -> nanocode_macro_support::Result<Tokens> {
        match variant.style {
            Style::Unit => Ok(quote!(generator.subschema_for::<()>())),
            Style::Newtype => {
                let ty = variant.fields[0].ty;
                Ok(quote!(generator.subschema_for::<#ty>()))
            }
            Style::Tuple => Ok(self.tuple(&variant.fields)),
            Style::Struct => self.object(&variant.fields, None, deny),
        }
    }

    fn flattened_variant(
        &self,
        variant: &VariantModel<'_>,
        deny: bool,
    ) -> nanocode_macro_support::Result<Option<Tokens>> {
        let private = &self.private;
        match variant.style {
            Style::Unit => Ok(None),
            Style::Newtype => {
                let ty = variant.fields[0].ty;
                Ok(Some(
                    quote!(<#ty as #private::JsonSchema>::json_schema(generator)),
                ))
            }
            Style::Tuple => Ok(Some(self.tuple(&variant.fields))),
            Style::Struct => self.object(&variant.fields, None, deny).map(Some),
        }
    }
}

fn field_default(
    field: &FieldModel<'_>,
    container_has_default: bool,
) -> nanocode_macro_support::Result<Option<Tokens>> {
    let attributes = &field.attributes;
    if attributes.skip_serializing || (attributes.default.is_none() && !container_has_default) {
        return Ok(None);
    }
    if attributes.serialize_with {
        return Err(Error::new_spanned(
            field.ty,
            "JsonSchema does not support serialize_with on a field with a default",
        ));
    }
    let ty = field.ty;
    let value = match &attributes.default {
        None => {
            let member = &field.member;
            quote!(container_default.#member)
        }
        Some(DefaultValue::Trait) => quote!(<#ty>::default()),
        Some(DefaultValue::Function(path)) => quote!(#path()),
    };
    Ok(Some(match &attributes.skip_serializing_if {
        Some(path) => quote! {{
            let default = #value;
            if #path(&default) {
                ::core::option::Option::None
            } else {
                ::core::option::Option::Some(default)
            }
        }},
        None => quote!(::core::option::Option::Some(#value)),
    }))
}

fn names(
    input: &DeriveInput,
    container: &ContainerAttributes,
    private: &Tokens,
) -> nanocode_macro_support::Result<(Tokens, Tokens)> {
    let base_name = container
        .rename
        .clone()
        .unwrap_or_else(|| unraw(&input.ident));
    let type_parameters: Vec<&Ident> = input
        .generics
        .type_params()
        .map(|parameter| &parameter.ident)
        .collect();
    let const_parameters: Vec<&Ident> = input
        .generics
        .const_params()
        .map(|parameter| &parameter.ident)
        .collect();
    let parameter_count = type_parameters.len() + const_parameters.len();
    if parameter_count == 0 || (container.rename.is_some() && !base_name.contains('{')) {
        return Ok((
            quote!(#base_name.to_owned()),
            quote! {
                ::std::borrow::Cow::Borrowed(::std::concat!(::std::module_path!(), "::", #base_name))
            },
        ));
    }
    if container.rename.is_some() {
        return Err(Error::new_spanned(
            &input.ident,
            "JsonSchema does not support a generic type renamed with a format string",
        ));
    }
    let format = format!(
        "{base_name}_for_{{}}{}",
        "_and_{}".repeat(parameter_count - 1)
    );
    Ok((
        quote! {
            ::std::format!(
                #format
                #(, <#type_parameters as #private::JsonSchema>::schema_name())*
                #(, #const_parameters)*
            )
        },
        quote! {
            ::std::borrow::Cow::Owned(::std::format!(
                ::std::concat!(::std::module_path!(), "::", #format)
                #(, <#type_parameters as #private::JsonSchema>::schema_id())*
                #(, #const_parameters)*
            ))
        },
    ))
}

pub fn expand(input: &DeriveInput, nanocode: &Tokens) -> nanocode_macro_support::Result<Tokens> {
    let private = quote!(#nanocode::__private::json_schema);
    let container = ContainerAttributes::parse(&input.attrs)?;
    let mut generics = input.generics.clone();
    for parameter in generics.type_params_mut() {
        let bound = parse_tokens::<TypeParamBound>(quote!(#private::JsonSchema))
            .expect("a JsonSchema trait path parses as a bound");
        parameter.bounds.push(bound);
    }
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let ident = &input.ident;
    if container.transparent {
        let Data::Struct(data) = &input.data else {
            return Err(Error::new_spanned(
                ident,
                "serde(transparent) needs a struct",
            ));
        };
        let Some(field) = data.fields.iter().next() else {
            return Err(Error::new_spanned(
                ident,
                "serde(transparent) needs a field",
            ));
        };
        let ty = &field.ty;
        return Ok(quote! {
            const _: () = {
                #[automatically_derived]
                impl #impl_generics #private::JsonSchema for #ident #type_generics #where_clause {
                    fn is_referenceable() -> bool {
                        <#ty as #private::JsonSchema>::is_referenceable()
                    }

                    fn schema_name() -> ::std::string::String {
                        <#ty as #private::JsonSchema>::schema_name()
                    }

                    fn schema_id() -> ::std::borrow::Cow<'static, str> {
                        <#ty as #private::JsonSchema>::schema_id()
                    }

                    fn json_schema(generator: &mut #private::SchemaGenerator) -> #private::Schema {
                        <#ty as #private::JsonSchema>::json_schema(generator)
                    }

                    fn non_optional_json_schema(generator: &mut #private::SchemaGenerator) -> #private::Schema {
                        <#ty as #private::JsonSchema>::non_optional_json_schema(generator)
                    }

                    fn is_option() -> bool {
                        <#ty as #private::JsonSchema>::is_option()
                    }
                }
            };
        });
    }
    let (schema_name, schema_id) = names(input, &container, &private)?;
    let generator = Generator {
        private: private.clone(),
        container: &container,
    };
    let schema = match &input.data {
        Data::Struct(data) => generator.structure(&data.fields)?,
        Data::Enum(data) => generator.enumeration(data)?,
        Data::Union(_) => {
            return Err(Error::new_spanned(
                ident,
                "JsonSchema does not support unions",
            ));
        }
    };
    let schema = Documentation::parse(&input.attrs).apply(&private, schema);
    Ok(quote! {
        const _: () = {
            #[automatically_derived]
            impl #impl_generics #private::JsonSchema for #ident #type_generics #where_clause {
                fn schema_name() -> ::std::string::String {
                    #schema_name
                }

                fn schema_id() -> ::std::borrow::Cow<'static, str> {
                    #schema_id
                }

                #[allow(unused_variables)]
                fn json_schema(generator: &mut #private::SchemaGenerator) -> #private::Schema {
                    #schema
                }
            }
        };
    })
}
