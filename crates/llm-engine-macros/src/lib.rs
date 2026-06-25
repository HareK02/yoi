//! llm-engine-macros - Procedural macros for Tool generation
//!
//! Provides `#[tool_registry]` and `#[tool]` macros to
//! automatically generate `Tool` trait implementations from user-defined methods.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, FnArg, ImplItem, ItemImpl, Lit, Meta, Pat, ReturnType, Type, parse_macro_input,
};

/// Macro applied to an `impl` block that generates tools from methods marked with `#[tool]`.
///
/// # Example
/// ```ignore
/// #[tool_registry]
/// impl MyApp {
///     /// Get user information
///     /// Retrieves a user from the database by their ID.
///     #[tool]
///     async fn get_user(&self, user_id: String) -> Result<User, Error> { ... }
/// }
/// ```
///
/// This generates:
/// - `GetUserArgs` struct (for arguments)
/// - `Tool_get_user` struct (Tool wrapper)
/// - `impl Tool for Tool_get_user`
/// - `impl MyApp { fn get_user_tool(&self) -> Tool_get_user }`
#[proc_macro_attribute]
pub fn tool_registry(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut impl_block = parse_macro_input!(item as ItemImpl);
    let self_ty = &impl_block.self_ty;

    let mut generated_items = Vec::new();

    for item in &mut impl_block.items {
        if let ImplItem::Fn(method) = item {
            // Look for #[tool] attribute
            let mut is_tool = false;

            // Iterate through attributes to check for tool and remove it
            method.attrs.retain(|attr| {
                if attr.path().is_ident("tool") {
                    is_tool = true;
                    false // Remove the attribute
                } else {
                    true
                }
            });

            if is_tool {
                let tool_impl = generate_tool_impl(self_ty, method);
                generated_items.push(tool_impl);
            }
        }
    }

    let expanded = quote! {
        #impl_block

        #(#generated_items)*
    };

    TokenStream::from(expanded)
}

/// Extract description from doc comments
fn extract_doc_comment(attrs: &[Attribute]) -> String {
    let mut lines = Vec::new();

    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let Meta::NameValue(meta) = &attr.meta {
                if let syn::Expr::Lit(expr_lit) = &meta.value {
                    if let Lit::Str(lit_str) = &expr_lit.lit {
                        let line = lit_str.value();
                        // Remove only the leading space (after ///)
                        let trimmed = line.strip_prefix(' ').unwrap_or(&line);
                        lines.push(trimmed.to_string());
                    }
                }
            }
        }
    }

    lines.join("\n")
}

/// Extract description from #[description = "..."] attribute
fn extract_description_attr(attrs: &[syn::Attribute]) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident("description")
            && let Meta::NameValue(meta) = &attr.meta
            && let syn::Expr::Lit(expr_lit) = &meta.value
            && let Lit::Str(lit_str) = &expr_lit.lit
        {
            return Some(lit_str.value());
        }
    }
    None
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

/// Generate Tool implementation from a method
fn generate_tool_impl(self_ty: &Type, method: &syn::ImplItemFn) -> proc_macro2::TokenStream {
    let sig = &method.sig;
    let method_name = &sig.ident;
    let tool_name = method_name.to_string();

    // Generate struct names (convert to PascalCase)
    let pascal_name = to_pascal_case(&method_name.to_string());
    let tool_struct_name = format_ident!("Tool{}", pascal_name);
    let args_struct_name = format_ident!("{}Args", pascal_name);
    let definition_name = format_ident!("{}_definition", method_name);

    // Get description from doc comments
    let description = extract_doc_comment(&method.attrs);
    let description = if description.is_empty() {
        format!("Tool: {}", tool_name)
    } else {
        description
    };

    // Parse method arguments (excluding self). A parameter typed as
    // ToolExecutionContext is supplied from the execution context and is not
    // exposed in the JSON input schema.
    let method_args: Vec<_> = sig
        .inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                Some(pat_type)
            } else {
                None // Exclude self
            }
        })
        .collect();
    let json_args: Vec<_> = method_args
        .iter()
        .copied()
        .filter(|pat_type| !is_tool_execution_context_type(pat_type.ty.as_ref()))
        .collect();

    // Generate argument struct fields
    let arg_fields: Vec<_> = json_args
        .iter()
        .map(|pat_type| {
            let pat = &pat_type.pat;
            let ty = &pat_type.ty;
            let desc = extract_description_attr(&pat_type.attrs);

            // Extract identifier from pattern
            let field_name = if let Pat::Ident(pat_ident) = pat.as_ref() {
                &pat_ident.ident
            } else {
                panic!("Only simple identifiers are supported for tool arguments");
            };

            // Convert #[description] to schemars doc if present
            if let Some(desc_str) = desc {
                quote! {
                    #[schemars(description = #desc_str)]
                    pub #field_name: #ty
                }
            } else {
                quote! {
                    pub #field_name: #ty
                }
            }
        })
        .collect();

    // Code to expand method arguments in execute
    let call_args: Vec<_> = method_args
        .iter()
        .map(|pat_type| {
            if is_tool_execution_context_type(pat_type.ty.as_ref()) {
                quote! { ctx.clone() }
            } else if let Pat::Ident(pat_ident) = pat_type.pat.as_ref() {
                let ident = &pat_ident.ident;
                quote! { args.#ident }
            } else {
                panic!("Only simple identifiers are supported");
            }
        })
        .collect();
    let method_call = if call_args.is_empty() {
        quote! { self.ctx.#method_name() }
    } else {
        quote! { self.ctx.#method_name(#(#call_args),*) }
    };

    // Check if method is async
    let is_async = sig.asyncness.is_some();

    // Parse return type and determine if Result
    let awaiter = if is_async {
        quote! { .await }
    } else {
        quote! {}
    };

    // Determine if return type is Result
    let result_handling = if is_result_type(&sig.output) {
        quote! {
            match result {
                Ok(val) => Ok(format!("{:?}", val).into()),
                Err(e) => Err(::llm_engine::tool::ToolError::ExecutionFailed(format!("{}", e))),
            }
        }
    } else {
        quote! {
            Ok(format!("{:?}", result).into())
        }
    };

    // Create empty Args struct if no arguments
    let args_struct_def = if arg_fields.is_empty() {
        quote! {
            #[derive(serde::Deserialize, schemars::JsonSchema)]
            struct #args_struct_name {}
        }
    } else {
        quote! {
            #[derive(serde::Deserialize, schemars::JsonSchema)]
            struct #args_struct_name {
                #(#arg_fields),*
            }
        }
    };

    // Execute body handling for no arguments case
    let execute_body = if json_args.is_empty() {
        quote! {
            // Allow empty JSON object even with no JSON arguments
            let _: #args_struct_name = serde_json::from_str(input_json)
                .unwrap_or(#args_struct_name {});

            let result = #method_call #awaiter;
            #result_handling
        }
    } else {
        quote! {
            let args: #args_struct_name = serde_json::from_str(input_json)
                .map_err(|e| ::llm_engine::tool::ToolError::InvalidArgument(e.to_string()))?;

            let result = #method_call #awaiter;
            #result_handling
        }
    };

    quote! {
        #args_struct_def

        #[derive(Clone)]
        pub struct #tool_struct_name {
            ctx: #self_ty,
        }

        #[async_trait::async_trait]
        impl ::llm_engine::tool::Tool for #tool_struct_name {
            async fn execute(&self, input_json: &str, ctx: ::llm_engine::tool::ToolExecutionContext) -> Result<::llm_engine::tool::ToolOutput, ::llm_engine::tool::ToolError> {
                let _ = &ctx;
                #execute_body
            }
        }

        impl #self_ty {
            /// Get ToolDefinition (for registering with Engine)
            pub fn #definition_name(&self) -> ::llm_engine::tool::ToolDefinition {
                let ctx = self.clone();
                ::std::sync::Arc::new(move || {
                    let schema = schemars::schema_for!(#args_struct_name);
                    let meta = ::llm_engine::tool::ToolMeta::new(#tool_name)
                        .description(#description)
                        .input_schema(serde_json::to_value(schema).unwrap_or(serde_json::json!({})));
                    let tool: ::std::sync::Arc<dyn ::llm_engine::tool::Tool> =
                        ::std::sync::Arc::new(#tool_struct_name { ctx: ctx.clone() });
                    (meta, tool)
                })
            }
        }
    }
}

/// Determine if return type is Result
fn is_result_type(return_type: &ReturnType) -> bool {
    match return_type {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => {
            // For Type::Path, check if last segment is "Result"
            if let Type::Path(type_path) = ty.as_ref() {
                if let Some(segment) = type_path.path.segments.last() {
                    return segment.ident == "Result";
                }
            }
            false
        }
    }
}

/// Convert snake_case to PascalCase
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

/// Marker attribute. Does nothing here as it's processed by `tool_registry`.
#[proc_macro_attribute]
pub fn tool(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Marker for argument attributes. Interpreted by `tool_registry` during parsing.
///
/// # Example
/// ```ignore
/// #[tool]
/// async fn get_user(
///     &self,
///     #[description = "The ID of the user to retrieve"] user_id: String
/// ) -> Result<User, Error> { ... }
/// ```
#[proc_macro_attribute]
pub fn description(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
