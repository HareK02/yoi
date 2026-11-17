//! Adapt Serde configuration declarations to Decodal's schema derive.
//!
//! Decodal 0.4's derive also generates a decoder and does not understand Option,
//! Serde names, or tagged enums. Generate schema-only fields from the *existing*
//! declarations; never duplicate their field lists. Runtime decoding stays Serde.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, LitStr, Meta, Type, parse_macro_input};

#[proc_macro_derive(ConfigSchema, attributes(config_schema))]
pub fn derive_schema(input: TokenStream) -> TokenStream {
    expand(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn serde_meta(attrs: &[syn::Attribute]) -> syn::Result<Vec<Meta>> {
    let mut result = Vec::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        result.extend(attr.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        )?);
    }
    Ok(result)
}

fn string_value(meta: &Meta) -> syn::Result<String> {
    if let Meta::NameValue(pair) = meta
        && let syn::Expr::Lit(expr) = &pair.value
        && let syn::Lit::Str(value) = &expr.lit
    {
        return Ok(value.value());
    }
    Err(syn::Error::new_spanned(meta, "expected a string attribute"))
}

fn fields_schema(fields: &Fields) -> syn::Result<proc_macro2::TokenStream> {
    let mut generated = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        let Some(ident) = &field.ident else {
            return Err(syn::Error::new_spanned(
                field,
                "schema requires named fields",
            ));
        };
        let mut name = ident.to_string().trim_start_matches("r#").to_string();
        let mut aliases = Vec::new();
        for meta in serde_meta(&field.attrs)? {
            if meta.path().is_ident("rename") {
                name = string_value(&meta)?;
            } else if meta.path().is_ident("alias") {
                aliases.push(string_value(&meta)?);
            } else if meta.path().is_ident("flatten")
                || meta.path().is_ident("skip")
                || meta.path().is_ident("skip_deserializing")
                || meta.path().is_ident("deserialize_with")
                || meta.path().is_ident("with")
            {
                return Err(syn::Error::new_spanned(
                    meta,
                    "custom field decoding needs an explicit schema adapter",
                ));
            }
        }
        let mut ty = field.ty.clone();
        for attr in field
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("config_schema"))
        {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("as_type") {
                    let value: LitStr = meta.value()?.parse()?;
                    ty = value.parse::<Type>()?;
                    Ok(())
                } else {
                    Err(meta.error("expected as_type = \"RustType\""))
                }
            })?;
        }
        for (alias_index, name) in std::iter::once(name).chain(aliases).enumerate() {
            let field_ident = format_ident!("field_{index}_{alias_index}");
            generated.push(quote! {
                #[decodal(rename = #name)]
                #field_ident: crate::config_schema::SchemaField<#ty>
            });
        }
    }
    Ok(quote! {{
        #[allow(dead_code)]
        #[derive(::decodal::Decodal)]
        struct Shape { #(#generated,)* }
        <Shape as ::decodal::DecodalSchema>::decodal_schema()
    }})
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            input.generics,
            "generic configuration declarations need an explicit adapter",
        ));
    }
    let meta = serde_meta(&input.attrs)?;
    let mut tag = None;
    for entry in &meta {
        if entry.path().is_ident("tag") {
            tag = Some(string_value(entry)?);
        }
        if entry.path().is_ident("untagged")
            || entry.path().is_ident("content")
            || entry.path().is_ident("from")
            || entry.path().is_ident("try_from")
            || entry.path().is_ident("transparent")
            || entry.path().is_ident("rename_all_fields")
        {
            return Err(syn::Error::new_spanned(
                entry,
                "union/custom decoding needs an explicit schema adapter",
            ));
        }
    }
    if let Data::Enum(data) = &input.data {
        for variant in &data.variants {
            for attr in serde_meta(&variant.attrs)? {
                if !attr.path().is_ident("rename") && !attr.path().is_ident("alias") {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "custom variant decoding needs an explicit schema adapter",
                    ));
                }
            }
        }
    }
    let schema = match &input.data {
        Data::Struct(data) => {
            if let Some(entry) = meta.iter().find(|m| m.path().is_ident("rename_all")) {
                return Err(syn::Error::new_spanned(
                    entry,
                    "struct rename_all needs explicit schema support",
                ));
            }
            fields_schema(&data.fields)?
        }
        Data::Enum(data) if tag.is_some() => {
            let tag = tag.unwrap();
            let variants = data
                .variants
                .iter()
                .map(|v| fields_schema(&v.fields))
                .collect::<syn::Result<Vec<_>>>()?;
            // Decodal has no union range. Expose the combined field shape, as the
            // former authoring schema did; Serde validates the selected variant.
            quote! {
                crate::config_schema::merge_shapes([
                    ::decodal::Value::object([(#tag, ::decodal::Value::string_type())]),
                    #(#variants,)*
                ])
            }
        }
        Data::Enum(data)
            if data
                .variants
                .iter()
                .all(|v| matches!(v.fields, Fields::Unit)) =>
        {
            quote!(::decodal::Value::string_type())
        }
        _ => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "unsupported schema shape; provide an explicit adapter",
            ));
        }
    };
    let name = input.ident;
    Ok(quote! {
        impl crate::config_schema::ConfigSchema for #name {
            fn config_schema() -> ::decodal::Value { #schema }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_serde_shapes_fail_instead_of_silently_omitting_fields() {
        for input in [
            syn::parse_quote!(
                #[serde(tag = "kind", rename_all_fields = "camelCase")]
                enum Bad {
                    A { some_field: bool },
                }
            ),
            syn::parse_quote!(
                #[serde(tag = "kind")]
                enum Bad {
                    #[serde(skip_deserializing)]
                    A,
                }
            ),
            syn::parse_quote!(
                struct Bad {
                    #[serde(flatten)]
                    inner: Inner,
                }
            ),
            syn::parse_quote!(
                struct Bad {
                    #[serde(skip)]
                    hidden: String,
                }
            ),
            syn::parse_quote!(
                struct Bad {
                    #[serde(deserialize_with = "custom")]
                    value: String,
                }
            ),
            syn::parse_quote!(
                #[serde(untagged)]
                enum Bad {
                    A(String),
                    B(bool),
                }
            ),
            syn::parse_quote!(
                #[serde(rename_all = "camelCase")]
                struct Bad {
                    some_field: bool,
                }
            ),
        ] {
            assert!(expand(input).is_err());
        }
    }

    #[test]
    fn schema_generation_uses_decodal_derive_and_serde_field_names() {
        let generated = expand(syn::parse_quote! {
            struct Config {
                #[serde(rename = "ref", alias = "reference", default)]
                source: Option<String>,
            }
        })
        .unwrap()
        .to_string();
        assert!(generated.contains("decodal :: Decodal"));
        assert!(generated.contains("rename = \"ref\""));
        assert!(generated.contains("rename = \"reference\""));
        assert!(generated.contains("SchemaField < Option < String > >"));
        // Serde defaults must not turn into materialized schema defaults.
        assert!(!generated.contains("default"));
    }
}
