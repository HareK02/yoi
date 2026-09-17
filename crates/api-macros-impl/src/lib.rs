//! Implementation of `api-macros`.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    Attribute, FnArg, GenericArgument, Ident, ItemTrait, LitInt, LitStr, Pat, PathArguments,
    ReturnType, Token, TraitItem, TraitItemFn, Type, Visibility,
    parse::{Parse, ParseStream},
    parse_macro_input,
    spanned::Spanned,
};

#[proc_macro_attribute]
pub fn api(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = parse_macro_input!(item as ItemTrait);

    normalize_api(attr, item)
        .and_then(expand_api)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    fn from_ident(ident: &Ident) -> Option<Self> {
        match ident.to_string().as_str() {
            "get" => Some(Self::Get),
            "post" => Some(Self::Post),
            "put" => Some(Self::Put),
            "patch" => Some(Self::Patch),
            "delete" => Some(Self::Delete),
            "head" => Some(Self::Head),
            "options" => Some(Self::Options),
            _ => None,
        }
    }

    fn tokens(self) -> proc_macro2::TokenStream {
        let variant = match self {
            Self::Get => quote!(Get),
            Self::Post => quote!(Post),
            Self::Put => quote!(Put),
            Self::Patch => quote!(Patch),
            Self::Delete => quote!(Delete),
            Self::Head => quote!(Head),
            Self::Options => quote!(Options),
        };
        quote!(::api_macros::HttpMethod::#variant)
    }

    fn permits_request_body(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch | Self::Delete)
    }
}

struct RouteArgs {
    path: LitStr,
    operation_id: Option<LitStr>,
    status: Option<LitInt>,
    error_status: Option<LitInt>,
}

impl Parse for RouteArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let path = input.parse()?;
        let mut result = Self {
            path,
            operation_id: None,
            status: None,
            error_status: None,
        };

        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "operation_id" => set_once(
                    &mut result.operation_id,
                    input.parse::<LitStr>()?,
                    &key,
                    "operation_id",
                )?,
                "status" => set_once(&mut result.status, input.parse::<LitInt>()?, &key, "status")?,
                "error_status" => set_once(
                    &mut result.error_status,
                    input.parse::<LitInt>()?,
                    &key,
                    "error_status",
                )?,
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unsupported route option; expected operation_id, status, or error_status",
                    ));
                }
            }
        }

        Ok(result)
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, key: &Ident, display: &str) -> syn::Result<()> {
    if slot.replace(value).is_some() {
        return Err(syn::Error::new(
            key.span(),
            format!("duplicate `{display}` route option"),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Location {
    Path,
    Query,
    Header,
    Body,
}

impl Location {
    fn tokens(self) -> proc_macro2::TokenStream {
        let variant = match self {
            Self::Path => quote!(Path),
            Self::Query => quote!(Query),
            Self::Header => quote!(Header),
            Self::Body => quote!(Body),
        };
        quote!(::api_macros::ParameterLocation::#variant)
    }
}

struct Parameter {
    rust_name: String,
    wire_name: String,
    location: Location,
    ty: Type,
}

struct Operation {
    method_ident: Ident,
    marker_ident: Ident,
    operation_id: String,
    method: Method,
    path: String,
    parameters: Vec<Parameter>,
    request_body: Option<Type>,
    response_body: Option<Type>,
    error_body: Option<Type>,
    response_status: u16,
    error_status: Option<u16>,
}

struct ApiDefinition {
    item: ItemTrait,
    metadata_ident: Ident,
    operations_module_ident: Ident,
    operations: Vec<Operation>,
}

fn normalize_api(
    attr: proc_macro2::TokenStream,
    mut item: ItemTrait,
) -> syn::Result<ApiDefinition> {
    if !attr.is_empty() {
        return Err(syn::Error::new(
            attr.span(),
            "api does not accept arguments",
        ));
    }
    if !matches!(item.vis, Visibility::Public(_)) {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "an #[api] trait must be public",
        ));
    }
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "an #[api] trait cannot be generic",
        ));
    }

    let trait_ident = item.ident.clone();
    let metadata_ident = format_ident!("{}Metadata", trait_ident);
    let operations_module_ident = format_ident!("{}_operations", to_snake_case(&trait_ident));
    let mut operations = Vec::new();
    let mut operation_ids = BTreeMap::<String, Span>::new();
    let mut routes = BTreeMap::<(Method, String), Span>::new();

    for trait_item in &mut item.items {
        let TraitItem::Fn(method) = trait_item else {
            return Err(syn::Error::new_spanned(
                trait_item,
                "an #[api] trait may contain methods only",
            ));
        };
        let operation = normalize_operation(method)?;

        if operation_ids
            .insert(operation.operation_id.clone(), method.sig.ident.span())
            .is_some()
        {
            return Err(syn::Error::new(
                method.sig.ident.span(),
                format!("duplicate operation ID `{}`", operation.operation_id),
            ));
        }
        if routes
            .insert(
                (operation.method, operation.path.clone()),
                method.sig.ident.span(),
            )
            .is_some()
        {
            return Err(syn::Error::new(
                method.sig.ident.span(),
                format!(
                    "duplicate route for {} `{}`",
                    method_name(operation.method),
                    operation.path
                ),
            ));
        }
        operations.push(operation);
    }

    if operations.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "an #[api] trait must declare at least one operation",
        ));
    }

    operations.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));

    Ok(ApiDefinition {
        item,
        metadata_ident,
        operations_module_ident,
        operations,
    })
}

fn normalize_operation(method: &mut TraitItemFn) -> syn::Result<Operation> {
    validate_signature(method)?;

    let mut route = None;
    let mut retained_attrs = Vec::new();
    for attr in std::mem::take(&mut method.attrs) {
        let maybe_method = attr.path().get_ident().and_then(Method::from_ident);
        if let Some(http_method) = maybe_method {
            if route.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "an API method must have exactly one HTTP method attribute",
                ));
            }
            let args = attr.parse_args::<RouteArgs>()?;
            route = Some((http_method, args));
        } else {
            retained_attrs.push(attr);
        }
    }
    method.attrs = retained_attrs;

    let (http_method, route) = route.ok_or_else(|| {
        syn::Error::new_spanned(
            &method.sig.ident,
            "an API method requires one of #[get], #[post], #[put], #[patch], #[delete], #[head], or #[options]",
        )
    })?;

    let path = route.path.value();
    let placeholders = parse_path_template(&route.path)?;
    let operation_id = route
        .operation_id
        .as_ref()
        .map(LitStr::value)
        .unwrap_or_else(|| method.sig.ident.to_string());
    validate_operation_id(
        &operation_id,
        route.operation_id.as_ref().unwrap_or(&route.path),
    )?;

    let (response_body, error_body) = parse_return_type(&method.sig.output)?;
    if let Some(ty) = response_body.as_ref() {
        validate_named_body_type(ty, "response")?;
    }
    if let Some(ty) = error_body.as_ref() {
        validate_named_body_type(ty, "error response")?;
    }

    let response_status = parse_status(route.status.as_ref(), "status")?
        .unwrap_or(if response_body.is_some() { 200 } else { 204 });
    if !(200..=299).contains(&response_status) {
        return Err(syn::Error::new(
            route
                .status
                .as_ref()
                .map_or(route.path.span(), Spanned::span),
            "status must be a success status between 200 and 299",
        ));
    }
    let error_status = parse_status(route.error_status.as_ref(), "error_status")?;
    if error_status.is_some() && error_body.is_none() {
        return Err(syn::Error::new_spanned(
            route.error_status,
            "error_status requires a public error response type",
        ));
    }
    if let Some(status) = error_status
        && status < 400
    {
        return Err(syn::Error::new_spanned(
            route.error_status,
            "error_status must be between 400 and 599",
        ));
    }
    let error_status = error_body.as_ref().map(|_| error_status.unwrap_or(400));

    if http_method == Method::Head && response_body.is_some() {
        return Err(syn::Error::new_spanned(
            &method.sig.output,
            "HEAD operations cannot declare a response body",
        ));
    }
    if matches!(response_status, 204 | 205) && response_body.is_some() {
        return Err(syn::Error::new_spanned(
            &method.sig.output,
            format!("status {response_status} cannot have a response body"),
        ));
    }

    let mut parameters = Vec::new();
    let mut body_type = None;
    let mut path_parameters = BTreeSet::new();
    for input in &mut method.sig.inputs {
        let FnArg::Typed(input) = input else {
            continue;
        };
        let ident = argument_ident(&input.pat)?;
        let rust_name = ident.to_string();
        let explicit = take_location(&mut input.attrs)?;
        let (location, wire_name) = match explicit {
            Some((Location::Path, wire_name)) => {
                if wire_name.is_some() {
                    return Err(syn::Error::new(
                        ident.span(),
                        "#[path] does not accept a wire name",
                    ));
                }
                (Location::Path, rust_name.clone())
            }
            Some((Location::Query, wire_name)) => {
                if wire_name.is_some() {
                    return Err(syn::Error::new(
                        ident.span(),
                        "#[query] does not accept a wire name",
                    ));
                }
                (Location::Query, rust_name.clone())
            }
            Some((Location::Header, wire_name)) => (
                Location::Header,
                wire_name.unwrap_or_else(|| rust_name.clone()),
            ),
            Some((Location::Body, wire_name)) => {
                if wire_name.is_some() {
                    return Err(syn::Error::new(
                        ident.span(),
                        "#[body] does not accept a wire name",
                    ));
                }
                (Location::Body, rust_name.clone())
            }
            None if placeholders.contains(&rust_name) => (Location::Path, rust_name.clone()),
            None => {
                return Err(syn::Error::new(
                    ident.span(),
                    "API arguments must use #[path], #[query], #[header], or #[body]; path arguments may omit #[path] when their name matches a placeholder",
                ));
            }
        };

        if matches!(location, Location::Path) {
            path_parameters.insert(rust_name.clone());
        }
        if matches!(location, Location::Body) {
            if body_type.is_some() {
                return Err(syn::Error::new(
                    ident.span(),
                    "an API operation may declare only one request body",
                ));
            }
            validate_named_body_type(&input.ty, "request")?;
            body_type = Some((*input.ty).clone());
        }
        parameters.push(Parameter {
            rust_name,
            wire_name,
            location,
            ty: (*input.ty).clone(),
        });
    }

    let missing: Vec<_> = placeholders.difference(&path_parameters).cloned().collect();
    if !missing.is_empty() {
        return Err(syn::Error::new(
            route.path.span(),
            format!(
                "path placeholder(s) have no matching path parameter: {}",
                missing.join(", ")
            ),
        ));
    }
    let extra: Vec<_> = path_parameters.difference(&placeholders).cloned().collect();
    if !extra.is_empty() {
        return Err(syn::Error::new(
            route.path.span(),
            format!(
                "path parameter(s) have no matching placeholder: {}",
                extra.join(", ")
            ),
        ));
    }
    if body_type.is_some() && !http_method.permits_request_body() {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            format!(
                "{} operations cannot declare a request body",
                method_name(http_method)
            ),
        ));
    }

    Ok(Operation {
        marker_ident: format_ident!("{}", to_pascal_case(&method.sig.ident)),
        method_ident: method.sig.ident.clone(),
        operation_id,
        method: http_method,
        path,
        parameters,
        request_body: body_type,
        response_body,
        error_body,
        response_status,
        error_status,
    })
}

fn validate_signature(method: &TraitItemFn) -> syn::Result<()> {
    let sig = &method.sig;
    if sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &sig.fn_token,
            "API methods must be async",
        ));
    }
    if sig.unsafety.is_some()
        || sig.abi.is_some()
        || sig.variadic.is_some()
        || !sig.generics.params.is_empty()
        || sig.generics.where_clause.is_some()
    {
        return Err(syn::Error::new_spanned(
            sig,
            "API methods cannot be unsafe, extern, variadic, or generic",
        ));
    }
    if method.default.is_some() {
        return Err(syn::Error::new_spanned(
            method,
            "API methods cannot provide a default implementation",
        ));
    }
    match sig.inputs.first() {
        Some(FnArg::Receiver(receiver))
            if receiver.reference.is_some() && receiver.mutability.is_none() => {}
        _ => {
            return Err(syn::Error::new_spanned(
                &sig.inputs,
                "API methods must take `&self` as their first argument",
            ));
        }
    }
    Ok(())
}

fn take_location(attrs: &mut Vec<Attribute>) -> syn::Result<Option<(Location, Option<String>)>> {
    let mut result = None;
    let mut retained = Vec::new();
    for attr in std::mem::take(attrs) {
        let location = match attr.path().get_ident().map(ToString::to_string).as_deref() {
            Some("path") => Some(Location::Path),
            Some("query") => Some(Location::Query),
            Some("header") => Some(Location::Header),
            Some("body") => Some(Location::Body),
            Some("stream") | Some("multipart") | Some("binary") => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "this wire kind is explicitly unsupported by the initial API contract",
                ));
            }
            _ => None,
        };
        let Some(location) = location else {
            retained.push(attr);
            continue;
        };
        if result.is_some() {
            return Err(syn::Error::new_spanned(
                attr,
                "an API argument may have only one location attribute",
            ));
        }
        let wire_name = if matches!(location, Location::Header) {
            if matches!(attr.meta, syn::Meta::Path(_)) {
                None
            } else {
                Some(attr.parse_args::<LitStr>()?.value())
            }
        } else if matches!(attr.meta, syn::Meta::Path(_)) {
            None
        } else {
            return Err(syn::Error::new_spanned(
                attr,
                "only #[header(\"wire-name\")] accepts an argument",
            ));
        };
        result = Some((location, wire_name));
    }
    *attrs = retained;
    Ok(result)
}

fn parse_path_template(path: &LitStr) -> syn::Result<BTreeSet<String>> {
    let value = path.value();
    if !value.starts_with('/') {
        return Err(syn::Error::new(
            path.span(),
            "API paths must start with `/`",
        ));
    }
    if value.contains('?') || value.contains('#') {
        return Err(syn::Error::new(
            path.span(),
            "API path templates cannot contain a query string or fragment",
        ));
    }

    let mut placeholders = BTreeSet::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => {
                let rest = &value[index + 1..];
                let Some(end) = rest.find('}') else {
                    return Err(syn::Error::new(path.span(), "unclosed path placeholder"));
                };
                let name = &rest[..end];
                if name.is_empty()
                    || !name.chars().enumerate().all(|(index, ch)| {
                        ch == '_'
                            || ch.is_ascii_alphanumeric() && (index > 0 || !ch.is_ascii_digit())
                    })
                {
                    return Err(syn::Error::new(
                        path.span(),
                        "path placeholders must be valid Rust identifiers",
                    ));
                }
                if !placeholders.insert(name.to_owned()) {
                    return Err(syn::Error::new(
                        path.span(),
                        format!("duplicate path placeholder `{name}`"),
                    ));
                }
                index += end + 2;
            }
            b'}' => {
                return Err(syn::Error::new(
                    path.span(),
                    "path template contains an unmatched `}`",
                ));
            }
            _ => index += 1,
        }
    }
    Ok(placeholders)
}

fn validate_operation_id(operation_id: &str, source: &impl Spanned) -> syn::Result<()> {
    if operation_id.is_empty()
        || !operation_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':' | '/'))
    {
        return Err(syn::Error::new(
            source.span(),
            "operation_id must be non-empty and contain only ASCII letters, digits, `.`, `-`, `_`, `:`, or `/`",
        ));
    }
    Ok(())
}

fn parse_status(value: Option<&LitInt>, name: &str) -> syn::Result<Option<u16>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let status = value.base10_parse::<u16>()?;
    if !(100..=599).contains(&status) {
        return Err(syn::Error::new(
            value.span(),
            format!("{name} must be an HTTP status between 100 and 599"),
        ));
    }
    Ok(Some(status))
}

fn parse_return_type(output: &ReturnType) -> syn::Result<(Option<Type>, Option<Type>)> {
    let ReturnType::Type(_, ty) = output else {
        return Ok((None, None));
    };
    if is_unit(ty) {
        return Ok((None, None));
    }

    if let Type::Path(type_path) = ty.as_ref()
        && let Some(last) = type_path.path.segments.last()
        && (last.ident == "Result" || last.ident == "ApiResult")
        && let PathArguments::AngleBracketed(arguments) = &last.arguments
    {
        let types: Vec<_> = arguments
            .args
            .iter()
            .filter_map(|argument| match argument {
                GenericArgument::Type(ty) => Some(ty.clone()),
                _ => None,
            })
            .collect();
        return match types.as_slice() {
            [response] => Ok((body_type(response), None)),
            [response, error] => Ok((body_type(response), body_type(error))),
            _ => Err(syn::Error::new_spanned(
                arguments,
                "API result types must have one response type and at most one error type",
            )),
        };
    }

    Ok((Some((**ty).clone()), None))
}

fn body_type(ty: &Type) -> Option<Type> {
    (!is_unit(ty)).then(|| ty.clone())
}

fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(tuple) if tuple.elems.is_empty())
}

fn validate_named_body_type(ty: &Type, kind: &str) -> syn::Result<()> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            format!("{kind} bodies must use a named Rust type"),
        ));
    };
    if path.qself.is_some() {
        return Err(syn::Error::new_spanned(
            ty,
            format!("{kind} bodies must use a direct named Rust type"),
        ));
    }
    let last = path.path.segments.last().expect("type path has a segment");
    let builtin = matches!(
        last.ident.to_string().as_str(),
        "bool"
            | "char"
            | "str"
            | "String"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "Vec"
            | "Option"
            | "Box"
    );
    if builtin {
        return Err(syn::Error::new_spanned(
            ty,
            format!("{kind} bodies must use an explicit named DTO type"),
        ));
    }
    Ok(())
}

fn argument_ident(pat: &Pat) -> syn::Result<&Ident> {
    let Pat::Ident(ident) = pat else {
        return Err(syn::Error::new_spanned(
            pat,
            "API arguments must use simple identifier patterns",
        ));
    };
    Ok(&ident.ident)
}

fn expand_api(api: ApiDefinition) -> syn::Result<proc_macro2::TokenStream> {
    let item = api.item;
    let metadata_ident = api.metadata_ident;
    let module_ident = api.operations_module_ident;

    let marker_items: Vec<_> = api
        .operations
        .iter()
        .map(|operation| {
            let marker_ident = &operation.marker_ident;
            let method_ident = &operation.method_ident;
            quote! {
                #[doc = concat!("Typed operation marker for Rust method `", stringify!(#method_ident), "`.")]
                #[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
                pub struct #marker_ident;
            }
        })
        .collect();
    let operation_impls: Vec<_> = api
        .operations
        .iter()
        .map(|operation| {
            let marker_ident = &operation.marker_ident;
            let metadata = metadata_tokens(operation);
            let parameter_types = operation.parameters.iter().map(|parameter| &parameter.ty);
            let request_type = operation
                .request_body
                .as_ref()
                .map(|ty| quote!(#ty))
                .unwrap_or_else(|| quote!(::api_macros::NoBody));
            let response_type = operation
                .response_body
                .as_ref()
                .map(|ty| quote!(#ty))
                .unwrap_or_else(|| quote!(::api_macros::NoBody));
            let error_type = operation
                .error_body
                .as_ref()
                .map(|ty| quote!(#ty))
                .unwrap_or_else(|| quote!(::api_macros::NoBody));
            quote! {
                impl ::api_macros::Operation for #module_ident::#marker_ident {
                    type Parameters = (#(#parameter_types,)*);
                    type RequestBody = #request_type;
                    type ResponseBody = #response_type;
                    type ErrorBody = #error_type;

                    const METADATA: ::api_macros::OperationMetadata = #metadata;
                }
            }
        })
        .collect();
    let inventory: Vec<_> = api.operations.iter().map(metadata_tokens).collect();

    Ok(quote! {
        #item

        #[doc = "Deterministic operation inventory for the API trait."]
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
        pub struct #metadata_ident;

        impl ::api_macros::ApiContract for #metadata_ident {
            const OPERATIONS: &'static [::api_macros::OperationMetadata] = &[
                #(#inventory),*
            ];
        }

        #[doc = "Typed markers generated for this API trait's operations."]
        pub mod #module_ident {
            #(#marker_items)*
        }

        #(#operation_impls)*
    })
}

fn metadata_tokens(operation: &Operation) -> proc_macro2::TokenStream {
    let operation_id = &operation.operation_id;
    let method = operation.method.tokens();
    let path = &operation.path;
    let request_kind = if operation.request_body.is_some() {
        quote!(::api_macros::WireKind::Json)
    } else {
        quote!(::api_macros::WireKind::Empty)
    };
    let response_kind = if operation.response_body.is_some() {
        quote!(::api_macros::WireKind::Json)
    } else {
        quote!(::api_macros::WireKind::Empty)
    };
    let response_status = operation.response_status;
    let error = match operation.error_status {
        Some(status) => quote! {
            ::core::option::Option::Some(::api_macros::ResponseMetadata {
                status: #status,
                body: ::api_macros::BodyMetadata {
                    wire_kind: ::api_macros::WireKind::Json,
                },
            })
        },
        None => quote!(::core::option::Option::None),
    };
    let parameters = operation.parameters.iter().map(|parameter| {
        let rust_name = &parameter.rust_name;
        let wire_name = &parameter.wire_name;
        let location = parameter.location.tokens();
        let _type_connection = &parameter.ty;
        quote! {
            ::api_macros::ParameterMetadata {
                rust_name: #rust_name,
                wire_name: #wire_name,
                location: #location,
            }
        }
    });

    quote! {
        ::api_macros::OperationMetadata {
            operation_id: #operation_id,
            method: #method,
            path: #path,
            parameters: &[#(#parameters),*],
            request_body: ::api_macros::BodyMetadata { wire_kind: #request_kind },
            response: ::api_macros::ResponseMetadata {
                status: #response_status,
                body: ::api_macros::BodyMetadata { wire_kind: #response_kind },
            },
            error_response: #error,
        }
    }
}

fn method_name(method: Method) -> &'static str {
    match method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Patch => "PATCH",
        Method::Delete => "DELETE",
        Method::Head => "HEAD",
        Method::Options => "OPTIONS",
    }
}

fn to_pascal_case(ident: &Ident) -> String {
    ident
        .to_string()
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn to_snake_case(ident: &Ident) -> String {
    let value = ident.to_string();
    let mut result = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}
