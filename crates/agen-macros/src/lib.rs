//! Procedural macros for declaring [`agen`](https://docs.rs/agen) tools.
//!
//! [`tool_registry`] expands methods marked with `#[tool]` into `agen::tool::Tool`
//! implementations and tool definitions. Applications normally use the re-exports from
//! `agen`; this companion crate exists so those macros can be published and versioned
//! independently.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, FnArg, ImplItem, ItemImpl, Lit, Meta, Pat, ReturnType, Type, parse_macro_input,
    spanned::Spanned,
};

/// Generates tools for methods marked with `#[tool]` in an `impl` block.
///
/// Method doc comments become the tool description. An argument can use
/// `#[description = "..."]` to supply its JSON Schema description.
///
/// ```ignore
/// #[derive(Clone)]
/// struct MyApp;
///
/// #[agen::tool_registry]
/// impl MyApp {
///     /// Retrieves a user by ID.
///     #[tool]
///     async fn get_user(
///         &self,
///         #[description = "The user ID"] user_id: String,
///     ) -> Result<String, std::io::Error> {
///         todo!()
///     }
/// }
/// ```
///
/// This generates a `ToolGetUser` wrapper, a `GetUserArgs` schema type, and
/// `MyApp::get_user_definition()`.
#[proc_macro_attribute]
pub fn tool_registry(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let impl_block = parse_macro_input!(item as ItemImpl);

    expand_tool_registry(attr, impl_block)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_tool_registry(
    attr: proc_macro2::TokenStream,
    mut impl_block: ItemImpl,
) -> syn::Result<proc_macro2::TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new(
            attr.span(),
            "tool_registry does not accept arguments",
        ));
    }

    let self_ty = impl_block.self_ty.as_ref().clone();
    let mut generated_items = Vec::new();

    for item in &mut impl_block.items {
        let ImplItem::Fn(method) = item else {
            continue;
        };

        let tool_attrs: Vec<_> = method
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("tool"))
            .collect();
        if tool_attrs.len() > 1 {
            return Err(syn::Error::new_spanned(
                tool_attrs[1],
                "duplicate #[tool] attribute",
            ));
        }
        let Some(tool_attr) = tool_attrs.first() else {
            continue;
        };
        if !matches!(tool_attr.meta, Meta::Path(_)) {
            return Err(syn::Error::new_spanned(
                tool_attr,
                "#[tool] does not accept arguments",
            ));
        }

        method.attrs.retain(|attr| !attr.path().is_ident("tool"));
        generated_items.push(generate_tool_impl(&self_ty, method)?);

        for input in &mut method.sig.inputs {
            if let FnArg::Typed(pat_type) = input {
                pat_type
                    .attrs
                    .retain(|attr| !attr.path().is_ident("description"));
            }
        }
    }

    Ok(quote! {
        #impl_block

        #(#generated_items)*
    })
}

fn extract_doc_comment(attrs: &[Attribute]) -> String {
    let mut lines = Vec::new();

    for attr in attrs {
        if attr.path().is_ident("doc")
            && let Meta::NameValue(meta) = &attr.meta
            && let syn::Expr::Lit(expr_lit) = &meta.value
            && let Lit::Str(lit_str) = &expr_lit.lit
        {
            let line = lit_str.value();
            let trimmed = line.strip_prefix(' ').unwrap_or(&line);
            lines.push(trimmed.to_string());
        }
    }

    lines.join("\n")
}

fn extract_description_attr(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut description = None;

    for attr in attrs
        .iter()
        .filter(|attr| attr.path().is_ident("description"))
    {
        let value = match &attr.meta {
            Meta::NameValue(meta) => match &meta.value {
                syn::Expr::Lit(expr_lit) => match &expr_lit.lit {
                    Lit::Str(value) => value.value(),
                    _ => {
                        return Err(syn::Error::new_spanned(
                            attr,
                            "description must be a string literal",
                        ));
                    }
                },
                _ => {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "description must be a string literal",
                    ));
                }
            },
            _ => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "expected #[description = \"...\"]",
                ));
            }
        };

        if description.replace(value).is_some() {
            return Err(syn::Error::new_spanned(
                attr,
                "duplicate #[description] attribute",
            ));
        }
    }

    Ok(description)
}

fn argument_ident(pat: &Pat) -> syn::Result<&syn::Ident> {
    match pat {
        Pat::Ident(pat_ident) => Ok(&pat_ident.ident),
        _ => Err(syn::Error::new_spanned(
            pat,
            "tool arguments must use simple identifier patterns",
        )),
    }
}

fn is_tool_execution_context_type(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "ToolExecutionContext")
}

fn generate_tool_impl(
    self_ty: &Type,
    method: &syn::ImplItemFn,
) -> syn::Result<proc_macro2::TokenStream> {
    let sig = &method.sig;
    let method_name = &sig.ident;
    let tool_name = method_name.to_string();

    let pascal_name = to_pascal_case(&method_name.to_string());
    let tool_struct_name = format_ident!("Tool{}", pascal_name);
    let args_struct_name = format_ident!("{}Args", pascal_name);
    let definition_name = format_ident!("{}_definition", method_name);

    let description = extract_doc_comment(&method.attrs);
    let description = if description.is_empty() {
        format!("Tool: {}", tool_name)
    } else {
        description
    };

    let method_args: Vec<_> = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pat_type) => Some(pat_type),
            FnArg::Receiver(_) => None,
        })
        .collect();
    let json_args: Vec<_> = method_args
        .iter()
        .copied()
        .filter(|pat_type| !is_tool_execution_context_type(pat_type.ty.as_ref()))
        .collect();

    let arg_fields: Vec<_> = json_args
        .iter()
        .map(|pat_type| {
            let field_name = argument_ident(pat_type.pat.as_ref())?;
            let ty = &pat_type.ty;
            let description = extract_description_attr(&pat_type.attrs)?;

            Ok(if let Some(description) = description {
                quote! {
                    #[schemars(description = #description)]
                    pub #field_name: #ty
                }
            } else {
                quote! {
                    pub #field_name: #ty
                }
            })
        })
        .collect::<syn::Result<_>>()?;

    let call_args: Vec<_> = method_args
        .iter()
        .map(|pat_type| {
            if is_tool_execution_context_type(pat_type.ty.as_ref()) {
                Ok(quote! { ctx.clone() })
            } else {
                let ident = argument_ident(pat_type.pat.as_ref())?;
                Ok(quote! { args.#ident })
            }
        })
        .collect::<syn::Result<_>>()?;
    let method_call = if call_args.is_empty() {
        quote! { self.ctx.#method_name() }
    } else {
        quote! { self.ctx.#method_name(#(#call_args),*) }
    };

    let awaiter = if sig.asyncness.is_some() {
        quote! { .await }
    } else {
        quote! {}
    };

    let result_handling = if is_result_type(&sig.output) {
        quote! {
            match result {
                Ok(val) => Ok(format!("{:?}", val).into()),
                Err(error) => Err(::agen::tool::ToolError::ExecutionFailed(format!("{}", error))),
            }
        }
    } else {
        quote! {
            Ok(format!("{:?}", result).into())
        }
    };

    let args_struct_def = quote! {
        #[derive(
            ::agen::__private::serde::Deserialize,
            ::agen::__private::schemars::JsonSchema,
        )]
        #[serde(crate = "::agen::__private::serde")]
        #[schemars(crate = "::agen::__private::schemars")]
        struct #args_struct_name {
            #(#arg_fields),*
        }
    };

    let execute_body = if json_args.is_empty() {
        quote! {
            let _: #args_struct_name = ::agen::__private::serde_json::from_str(input_json)
                .unwrap_or(#args_struct_name {});

            let result = #method_call #awaiter;
            #result_handling
        }
    } else {
        quote! {
            let args: #args_struct_name = ::agen::__private::serde_json::from_str(input_json)
                .map_err(|error| ::agen::tool::ToolError::InvalidArgument(error.to_string()))?;

            let result = #method_call #awaiter;
            #result_handling
        }
    };

    Ok(quote! {
        #args_struct_def

        #[derive(Clone)]
        pub struct #tool_struct_name {
            ctx: #self_ty,
        }

        #[::agen::__private::async_trait::async_trait]
        impl ::agen::tool::Tool for #tool_struct_name {
            async fn execute(
                &self,
                input_json: &str,
                ctx: ::agen::tool::ToolExecutionContext,
            ) -> Result<::agen::tool::ToolOutput, ::agen::tool::ToolError> {
                let _ = &ctx;
                #execute_body
            }
        }

        impl #self_ty {
            /// Returns a tool definition for registration with an `agen::Engine`.
            pub fn #definition_name(&self) -> ::agen::tool::ToolDefinition {
                let ctx = self.clone();
                ::std::sync::Arc::new(move || {
                    let schema = ::agen::__private::schemars::schema_for!(#args_struct_name);
                    let meta = ::agen::tool::ToolMeta::new(#tool_name)
                        .description(#description)
                        .input_schema(
                            ::agen::__private::serde_json::to_value(schema)
                                .unwrap_or_else(|_| ::agen::__private::serde_json::json!({})),
                        );
                    let tool: ::std::sync::Arc<dyn ::agen::tool::Tool> =
                        ::std::sync::Arc::new(#tool_struct_name { ctx: ctx.clone() });
                    (meta, tool)
                })
            }
        }
    })
}

fn is_result_type(return_type: &ReturnType) -> bool {
    match return_type {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => {
            if let Type::Path(type_path) = ty.as_ref()
                && let Some(segment) = type_path.path.segments.last()
            {
                return segment.ident == "Result";
            }
            false
        }
    }
}

fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect()
}

/// Marker attribute interpreted by [`tool_registry`].
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    marker_attribute("tool", attr, item)
}

/// Argument description marker interpreted by [`tool_registry`].
///
/// Use it as `#[description = "The argument description"]` on a tool method argument.
#[proc_macro_attribute]
pub fn description(attr: TokenStream, item: TokenStream) -> TokenStream {
    marker_attribute("description", attr, item)
}

fn marker_attribute(name: &str, attr: TokenStream, item: TokenStream) -> TokenStream {
    if attr.is_empty() {
        item
    } else {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("{name} is a marker interpreted by #[tool_registry]"),
        )
        .into_compile_error()
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;
    use syn::parse_quote;

    #[test]
    fn rejects_tool_registry_arguments() {
        let implementation: ItemImpl = parse_quote!(impl Registry {});
        let error = expand_tool_registry(quote!(unexpected), implementation).unwrap_err();

        assert!(error.to_string().contains("does not accept arguments"));
    }

    #[test]
    fn rejects_duplicate_tool_markers() {
        let implementation: ItemImpl = parse_quote! {
            impl Registry {
                #[tool]
                #[tool]
                fn inspect(&self) {}
            }
        };
        let error = expand_tool_registry(quote!(), implementation).unwrap_err();

        assert!(error.to_string().contains("duplicate #[tool]"));
    }

    #[test]
    fn rejects_invalid_description_attributes() {
        let implementation: ItemImpl = parse_quote! {
            impl Registry {
                #[tool]
                fn inspect(&self, #[description] input: String) {}
            }
        };
        let error = expand_tool_registry(quote!(), implementation).unwrap_err();

        assert!(error.to_string().contains("expected #[description"));
    }

    #[test]
    fn rejects_duplicate_description_attributes() {
        let implementation: ItemImpl = parse_quote! {
            impl Registry {
                #[tool]
                fn inspect(
                    &self,
                    #[description = "first"]
                    #[description = "second"]
                    input: String,
                ) {}
            }
        };
        let error = expand_tool_registry(quote!(), implementation).unwrap_err();

        assert!(error.to_string().contains("duplicate #[description]"));
    }

    #[test]
    fn generated_code_uses_only_agen_runtime_paths() {
        let implementation: ItemImpl = parse_quote! {
            impl Registry {
                #[tool]
                fn inspect(&self, input: String) -> Result<String, Error> {
                    unreachable!()
                }
            }
        };
        let expanded = expand_tool_registry(quote!(), implementation)
            .unwrap()
            .to_string();

        assert!(expanded.contains(":: agen :: tool :: Tool"));
        assert!(expanded.contains(":: agen :: __private :: serde_json"));
        assert!(expanded.contains(":: agen :: __private :: serde"));
        assert!(expanded.contains(":: agen :: __private :: schemars"));
    }
}
