//! Implementation of `api-macros`.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    Attribute, FnArg, GenericArgument, Ident, ItemTrait, LitInt, LitStr, Pat, PathArguments,
    ReturnType, Token, TraitItem, TraitItemFn, Type, Visibility,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
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

#[derive(Clone, Copy, Default)]
struct AdapterConfig {
    reqwest: bool,
    axum: bool,
}

impl Parse for AdapterConfig {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let adapters = Punctuated::<Ident, Token![,]>::parse_terminated(input)?;
        let mut config = Self::default();
        for adapter in adapters {
            match adapter.to_string().as_str() {
                "reqwest" if !config.reqwest => config.reqwest = true,
                "axum" if !config.axum => config.axum = true,
                "reqwest" | "axum" => {
                    return Err(syn::Error::new(adapter.span(), "duplicate API adapter"));
                }
                _ => {
                    return Err(syn::Error::new(
                        adapter.span(),
                        "unsupported API adapter; expected `reqwest` or `axum`",
                    ));
                }
            }
        }
        Ok(config)
    }
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

    fn tokens(self, api_crate: &proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        let variant = match self {
            Self::Get => quote!(Get),
            Self::Post => quote!(Post),
            Self::Put => quote!(Put),
            Self::Patch => quote!(Patch),
            Self::Delete => quote!(Delete),
            Self::Head => quote!(Head),
            Self::Options => quote!(Options),
        };
        quote!(#api_crate::HttpMethod::#variant)
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
    fn tokens(self, api_crate: &proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        let variant = match self {
            Self::Path => quote!(Path),
            Self::Query => quote!(Query),
            Self::Header => quote!(Header),
            Self::Body => quote!(Body),
        };
        quote!(#api_crate::ParameterLocation::#variant)
    }
}

struct Parameter {
    rust_ident: Ident,
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
    fallible: bool,
    response_status: u16,
    error_status: Option<u16>,
}

struct ApiDefinition {
    item: ItemTrait,
    metadata_ident: Ident,
    operations_module_ident: Ident,
    operations: Vec<Operation>,
    adapters: AdapterConfig,
}

fn normalize_api(
    attr: proc_macro2::TokenStream,
    mut item: ItemTrait,
) -> syn::Result<ApiDefinition> {
    let adapters = syn::parse2::<AdapterConfig>(attr)?;
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
    let metadata_ident = format_ident!("{}Metadata", ident_text(&trait_ident));
    let operations_module_ident = format_ident!("{}_operations", to_snake_case(&trait_ident));
    let mut operations = Vec::new();
    let mut operation_ids = BTreeMap::<String, Span>::new();
    let mut routes = BTreeMap::<(Method, String), Span>::new();
    let mut marker_names = BTreeMap::<String, (String, Span)>::new();

    for trait_item in &mut item.items {
        let TraitItem::Fn(method) = trait_item else {
            return Err(syn::Error::new_spanned(
                trait_item,
                "an #[api] trait may contain methods only",
            ));
        };
        let operation = normalize_operation(method)?;

        let marker_name = operation.marker_ident.to_string();
        if let Some((first_method, first_span)) = marker_names.insert(
            marker_name.clone(),
            (ident_text(&method.sig.ident), method.sig.ident.span()),
        ) {
            let mut error = syn::Error::new(
                method.sig.ident.span(),
                format!(
                    "generated operation marker `{marker_name}` collides with method `{first_method}`"
                ),
            );
            error.combine(syn::Error::new(
                first_span,
                format!("first method generating marker `{marker_name}`"),
            ));
            return Err(error);
        }

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

    if adapters.axum || adapters.reqwest {
        make_service_futures_send(&mut item)?;
    }

    Ok(ApiDefinition {
        item,
        metadata_ident,
        operations_module_ident,
        operations,
        adapters,
    })
}

fn make_service_futures_send(item: &mut ItemTrait) -> syn::Result<()> {
    for trait_item in &mut item.items {
        let TraitItem::Fn(method) = trait_item else {
            continue;
        };
        method.sig.asyncness = None;
        let output = match &method.sig.output {
            ReturnType::Default => quote!(()),
            ReturnType::Type(_, ty) => quote!(#ty),
        };
        method.sig.output = syn::parse2(quote!(
            -> impl ::core::future::Future<Output = #output> + Send
        ))?;
    }
    Ok(())
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
        .unwrap_or_else(|| ident_text(&method.sig.ident));
    validate_operation_id(
        &operation_id,
        route.operation_id.as_ref().unwrap_or(&route.path),
    )?;

    let (response_body, error_body, fallible) = parse_return_type(&method.sig.output)?;
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
        let rust_name = ident_text(ident);
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
            rust_ident: ident.clone(),
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
        marker_ident: operation_marker_ident(&method.sig.ident),
        method_ident: method.sig.ident.clone(),
        operation_id,
        method: http_method,
        path,
        parameters,
        request_body: body_type,
        response_body,
        error_body,
        fallible,
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

fn parse_return_type(output: &ReturnType) -> syn::Result<(Option<Type>, Option<Type>, bool)> {
    let ReturnType::Type(_, ty) = output else {
        return Ok((None, None, false));
    };
    if is_unit(ty) {
        return Ok((None, None, false));
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
            [response] => Ok((body_type(response), None, true)),
            [response, error] => Ok((body_type(response), body_type(error), true)),
            _ => Err(syn::Error::new_spanned(
                arguments,
                "API result types must have one response type and at most one error type",
            )),
        };
    }

    Ok((Some((**ty).clone()), None, false))
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

fn api_crate_path() -> syn::Result<proc_macro2::TokenStream> {
    match crate_name("api-macros") {
        Ok(FoundCrate::Itself) => Ok(quote!(::api_macros)),
        Ok(FoundCrate::Name(name)) => {
            let ident = Ident::new(&name, Span::call_site());
            Ok(quote!(::#ident))
        }
        Err(error) => Err(syn::Error::new(
            Span::call_site(),
            format!("could not resolve the api-macros support crate: {error}"),
        )),
    }
}

fn reqwest_method_tokens(
    method: Method,
    api_crate: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let method = Ident::new(method_name(method), Span::call_site());
    quote!(#api_crate::reqwest::framework::Method::#method)
}

fn reqwest_path_segment_tokens(segment: &str, operation: &Operation) -> proc_macro2::TokenStream {
    let mut pieces = Vec::new();
    let mut remaining = segment;
    while let Some(start) = remaining.find('{') {
        let literal = &remaining[..start];
        if !literal.is_empty() {
            pieces.push(quote!(__segment.push_str(#literal);));
        }
        let after = &remaining[start + 1..];
        let end = after
            .find('}')
            .expect("path placeholders are validated before adapter generation");
        let name = &after[..end];
        let parameter = operation
            .parameters
            .iter()
            .find(|parameter| {
                matches!(parameter.location, Location::Path) && parameter.rust_name == name
            })
            .expect("path parameter names are validated before adapter generation");
        let ident = &parameter.rust_ident;
        pieces.push(quote! {
            __segment.push_str(&::std::string::ToString::to_string(&#ident));
        });
        remaining = &after[end + 1..];
    }
    if !remaining.is_empty() {
        pieces.push(quote!(__segment.push_str(#remaining);));
    }
    quote! {{
        let mut __segment = ::std::string::String::new();
        #(#pieces)*
        __segment
    }}
}

fn reqwest_adapter_tokens(
    api: &ApiDefinition,
    api_crate: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    if !api.adapters.reqwest {
        return quote!();
    }
    let trait_ident = &api.item.ident;
    let visibility = &api.item.vis;
    let client_ident = format_ident!("{}Client", ident_text(trait_ident));
    let builder_ident = format_ident!("{}ClientBuilder", ident_text(trait_ident));
    let methods = api.operations.iter().map(|operation| {
        let method_ident = &operation.method_ident;
        let arguments = operation.parameters.iter().map(|parameter| {
            let ident = &parameter.rust_ident;
            let ty = &parameter.ty;
            quote!(#ident: #ty)
        });
        let path_segments = operation
            .path
            .split('/')
            .skip(1)
            .map(|segment| reqwest_path_segment_tokens(segment, operation));
        let queries = operation.parameters.iter().filter_map(|parameter| {
            if !matches!(parameter.location, Location::Query) {
                return None;
            }
            let ident = &parameter.rust_ident;
            Some(quote! {
                #api_crate::reqwest::append_query(&mut __url, &#ident)
                    .map_err(#api_crate::reqwest::ClientError::from)?;
            })
        });
        let headers = operation.parameters.iter().filter_map(|parameter| {
            if !matches!(parameter.location, Location::Header) {
                return None;
            }
            let ident = &parameter.rust_ident;
            let name = &parameter.wire_name;
            Some(quote! {
                #api_crate::reqwest::insert_header(&mut __headers, #name, &#ident)
                    .map_err(#api_crate::reqwest::ClientError::from)?;
            })
        });
        let body = operation
            .parameters
            .iter()
            .find(|parameter| matches!(parameter.location, Location::Body))
            .map(|parameter| {
                let ident = &parameter.rust_ident;
                quote! {
                    ::core::option::Option::Some(
                        #api_crate::reqwest::encode_json(&#ident)
                            .map_err(#api_crate::reqwest::ClientError::from)?,
                    )
                }
            })
            .unwrap_or_else(|| quote!(::core::option::Option::None));
        let method = reqwest_method_tokens(operation.method, api_crate);
        let response_status = operation.response_status;
        let response_type = operation
            .response_body
            .as_ref()
            .map(|ty| quote!(#ty))
            .unwrap_or_else(|| quote!(()));
        let error_type = operation
            .error_body
            .as_ref()
            .map(|ty| quote!(#ty))
            .unwrap_or_else(|| quote!(#api_crate::NoBody));
        let success = if let Some(response) = operation.response_body.as_ref() {
            quote! {
                return __response
                    .decode_json::<#response>(#api_crate::reqwest::DecodeKind::Success)
                    .map_err(#api_crate::reqwest::ClientError::from);
            }
        } else {
            quote! {
                return __response
                    .require_empty()
                    .map_err(#api_crate::reqwest::ClientError::from);
            }
        };
        let public_error = match operation.error_body.as_ref() {
            Some(error) => quote! {
                if (400..=599).contains(&__actual) {
                    let __status = __response.status();
                    let __error = __response
                        .decode_json::<#error>(#api_crate::reqwest::DecodeKind::PublicError)
                        .map_err(|_| #api_crate::reqwest::ClientError::failure(
                            #api_crate::reqwest::ClientFailure::ErrorResponseDecode {
                                status: __status.as_u16(),
                            },
                        ))?;
                    return ::core::result::Result::Err(
                        #api_crate::reqwest::ClientError::public(__status, __error),
                    );
                }
            },
            None => quote!(),
        };
        quote! {
            pub async fn #method_ident(
                &self,
                #(#arguments),*
            ) -> ::core::result::Result<#response_type, #api_crate::reqwest::ClientError<#error_type>> {
                let __segments = [#(#path_segments),*];
                let mut __url = self.core.endpoint(&__segments)
                    .map_err(#api_crate::reqwest::ClientError::from)?;
                #(#queries)*
                let mut __headers = #api_crate::reqwest::framework::header::HeaderMap::new();
                #(#headers)*
                let __body = #body;
                let __response = self.core.send(#method, __url, __headers, __body)
                    .await
                    .map_err(#api_crate::reqwest::ClientError::from)?;
                let __actual = __response.status().as_u16();
                if __actual == #response_status {
                    #success
                }
                #public_error
                ::core::result::Result::Err(#api_crate::reqwest::ClientError::from(
                    #api_crate::reqwest::ClientFailure::UnexpectedStatus {
                        expected: #response_status,
                        actual: __actual,
                    },
                ))
            }
        }
    });

    quote! {
        #[doc = concat!("Typed Reqwest client for [`", stringify!(#trait_ident), "`].")]
        #visibility struct #client_ident<A = #api_crate::reqwest::NoAuthorizer> {
            core: #api_crate::reqwest::ClientCore<A>,
        }

        impl #client_ident<#api_crate::reqwest::NoAuthorizer> {
            pub fn try_new(
                base_url: impl ::core::convert::AsRef<str>,
            ) -> ::core::result::Result<Self, #api_crate::reqwest::ClientBuildError> {
                Self::builder(base_url)?.build()
            }

            pub fn builder(
                base_url: impl ::core::convert::AsRef<str>,
            ) -> ::core::result::Result<#builder_ident, #api_crate::reqwest::ClientBuildError> {
                Ok(#builder_ident {
                    inner: #api_crate::reqwest::ClientBuilder::try_new(base_url)?,
                })
            }

            pub fn with_client(
                base_url: #api_crate::reqwest::BaseUrl,
                client: #api_crate::reqwest::framework::Client,
            ) -> Self {
                #builder_ident {
                    inner: #api_crate::reqwest::ClientBuilder::from_base_url(base_url).client(client),
                }
                .build()
                .expect("an injected Reqwest client requires no fallible construction")
            }
        }

        impl<A> #client_ident<A> {
            pub fn from_core(core: #api_crate::reqwest::ClientCore<A>) -> Self {
                Self { core }
            }
        }

        impl<A: #api_crate::reqwest::RequestAuthorizer> #client_ident<A> {
            #(#methods)*
        }

        impl<A> ::core::fmt::Debug for #client_ident<A> {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter
                    .debug_struct(stringify!(#client_ident))
                    .field("core", &self.core)
                    .finish()
            }
        }

        #[doc = concat!("Builder for [`", stringify!(#client_ident), "`].")]
        #visibility struct #builder_ident<A = #api_crate::reqwest::NoAuthorizer> {
            inner: #api_crate::reqwest::ClientBuilder<A>,
        }

        impl<A> #builder_ident<A> {
            pub fn client(mut self, client: #api_crate::reqwest::framework::Client) -> Self {
                self.inner = self.inner.client(client);
                self
            }

            pub fn authorizer<B>(self, authorizer: B) -> #builder_ident<B> {
                #builder_ident { inner: self.inner.authorizer(authorizer) }
            }

            pub fn request_timeout(mut self, timeout: ::std::time::Duration) -> Self {
                self.inner = self.inner.request_timeout(timeout);
                self
            }

            pub fn response_body_limit(mut self, limit: usize) -> Self {
                self.inner = self.inner.response_body_limit(limit);
                self
            }

            pub fn build(
                self,
            ) -> ::core::result::Result<#client_ident<A>, #api_crate::reqwest::ClientBuildError> {
                self.inner.build().map(#client_ident::from_core)
            }
        }

        impl<A> ::core::fmt::Debug for #builder_ident<A> {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter
                    .debug_struct(stringify!(#builder_ident))
                    .field("inner", &self.inner)
                    .finish()
            }
        }
    }
}

fn axum_routing_tokens(
    method: Method,
    api_crate: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let function = match method {
        Method::Get => quote!(get),
        Method::Post => quote!(post),
        Method::Put => quote!(put),
        Method::Patch => quote!(patch),
        Method::Delete => quote!(delete),
        Method::Head => quote!(head),
        Method::Options => quote!(options),
    };
    quote!(#api_crate::axum::framework::#function)
}

fn ordered_path_parameters<'a>(operation: &'a Operation) -> Vec<&'a Parameter> {
    let mut result = Vec::new();
    let mut remaining = operation.path.as_str();
    while let Some(start) = remaining.find('{') {
        let after = &remaining[start + 1..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        if let Some(parameter) = operation.parameters.iter().find(|parameter| {
            matches!(parameter.location, Location::Path) && parameter.rust_name == name
        }) {
            result.push(parameter);
        }
        remaining = &after[end + 1..];
    }
    result
}

fn axum_adapter_tokens(
    api: &ApiDefinition,
    api_crate: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    if !api.adapters.axum {
        return quote!();
    }
    let trait_ident = &api.item.ident;
    let visibility = &api.item.vis;
    let adapter_ident = format_ident!("{}Axum", ident_text(trait_ident));
    let module_ident = format_ident!("{}_axum", to_snake_case(trait_ident));
    let route_functions: Vec<_> = api.operations.iter().map(|operation| {
        let method_ident = &operation.method_ident;
        let handler_ident = format_ident!("{}_handler", ident_text(method_ident));
        let route_path = &operation.path;
        let routing = axum_routing_tokens(operation.method, api_crate);
        let path_parameters = ordered_path_parameters(operation);
        let path_extractor = match path_parameters.as_slice() {
            [] => quote!(),
            [parameter] => {
                let ident = &parameter.rust_ident;
                let ty = &parameter.ty;
                quote!(#api_crate::axum::framework::Path(#ident): #api_crate::axum::framework::Path<#ty>,)
            }
            parameters => {
                let identifiers = parameters.iter().map(|parameter| &parameter.rust_ident);
                let types = parameters.iter().map(|parameter| &parameter.ty);
                quote!(#api_crate::axum::framework::Path((#(#identifiers),*)): #api_crate::axum::framework::Path<(#(#types),*)>,)
            }
        };
        let query_extractors = operation.parameters.iter().filter_map(|parameter| {
            if !matches!(parameter.location, Location::Query) {
                return None;
            }
            let ident = &parameter.rust_ident;
            let ty = &parameter.ty;
            Some(quote!(#api_crate::axum::framework::Query(#ident): #api_crate::axum::framework::Query<#ty>,))
        });
        let has_headers = operation.parameters.iter().any(|parameter| matches!(parameter.location, Location::Header));
        let header_extractor = has_headers.then(|| quote!(__headers: #api_crate::axum::framework::HeaderMap,));
        let header_parsers = operation.parameters.iter().filter_map(|parameter| {
            if !matches!(parameter.location, Location::Header) {
                return None;
            }
            let ident = &parameter.rust_ident;
            let ty = &parameter.ty;
            let name = &parameter.wire_name;
            Some(quote! {
                let #ident: #ty = match #api_crate::axum::parse_header(&__headers, #name) {
                    Ok(value) => value,
                    Err(status) => return #api_crate::axum::rejection(status),
                };
            })
        });
        let body_extractor = operation.parameters.iter().find_map(|parameter| {
            if !matches!(parameter.location, Location::Body) {
                return None;
            }
            let ident = &parameter.rust_ident;
            let ty = &parameter.ty;
            Some(quote!(#api_crate::axum::framework::Json(#ident): #api_crate::axum::framework::Json<#ty>,))
        });
        let call_arguments = operation.parameters.iter().map(|parameter| &parameter.rust_ident);
        let response_status = operation.response_status;
        let success = if operation.response_body.is_some() {
            quote!(#api_crate::axum::json_response(#api_crate::axum::status(#response_status), value))
        } else {
            quote!({ let _ = value; #api_crate::axum::empty_response(#api_crate::axum::status(#response_status)) })
        };
        let invocation = quote!(#trait_ident::#method_ident(&*__service, #(#call_arguments),*).await);
        let mapped = if operation.fallible {
            let error = match operation.error_status {
                Some(_) => quote!({
                    let status = #api_crate::HttpError::status_code(&error);
                    #api_crate::axum::json_response(#api_crate::axum::status(status), error)
                }),
                None => quote!({ let _ = error; #api_crate::axum::empty_response(#api_crate::axum::framework::StatusCode::INTERNAL_SERVER_ERROR) }),
            };
            quote! {
                match #invocation {
                    Ok(value) => #success,
                    Err(error) => #error,
                }
            }
        } else {
            quote! {
                let value = #invocation;
                #success
            }
        };
        quote! {
            pub fn #method_ident<S>(service: ::std::sync::Arc<S>) -> #api_crate::axum::framework::Router
            where
                S: super::#trait_ident + Send + Sync + 'static,
            {
                #api_crate::axum::framework::Router::new()
                    .route(#route_path, #routing(#handler_ident::<S>))
                    .with_state(service)
            }

            async fn #handler_ident<S>(
                #api_crate::axum::framework::State(__service): #api_crate::axum::framework::State<::std::sync::Arc<S>>,
                #path_extractor
                #(#query_extractors)*
                #header_extractor
                #body_extractor
            ) -> #api_crate::axum::framework::Response
            where
                S: super::#trait_ident + Send + Sync + 'static,
            {
                #(#header_parsers)*
                #mapped
            }
        }
    }).collect();
    let operation_merges = api.operations.iter().map(|operation| {
        let method_ident = &operation.method_ident;
        quote!(__router = __router.merge(#module_ident::#method_ident(__service.clone()));)
    });

    quote! {
        #[doc = concat!("Per-operation Axum routers for [`", stringify!(#trait_ident), "`].")]
        #visibility mod #module_ident {
            use super::*;
            #(#route_functions)*
        }

        #[doc = concat!("Axum router adapter for [`", stringify!(#trait_ident), "`].")]
        #[derive(Clone, Copy, Debug, Default)]
        #visibility struct #adapter_ident;

        impl #adapter_ident {
            pub fn router<S>(service: S) -> #api_crate::axum::framework::Router
            where
                S: #trait_ident + Send + Sync + 'static,
            {
                let __service = ::std::sync::Arc::new(service);
                let mut __router = #api_crate::axum::framework::Router::new();
                #(#operation_merges)*
                __router
            }
        }
    }
}

fn expand_api(api: ApiDefinition) -> syn::Result<proc_macro2::TokenStream> {
    let api_crate = api_crate_path()?;
    let reqwest_adapter = reqwest_adapter_tokens(&api, &api_crate);
    let axum_adapter = axum_adapter_tokens(&api, &api_crate);
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
            let metadata = metadata_tokens(operation, &api_crate);
            let parameter_types = operation.parameters.iter().map(|parameter| &parameter.ty);
            let request_type = operation
                .request_body
                .as_ref()
                .map(|ty| quote!(#ty))
                .unwrap_or_else(|| quote!(#api_crate::NoBody));
            let response_type = operation
                .response_body
                .as_ref()
                .map(|ty| quote!(#ty))
                .unwrap_or_else(|| quote!(#api_crate::NoBody));
            let error_type = operation
                .error_body
                .as_ref()
                .map(|ty| quote!(#ty))
                .unwrap_or_else(|| quote!(#api_crate::NoBody));
            quote! {
                impl #api_crate::Operation for #module_ident::#marker_ident {
                    type Parameters = (#(#parameter_types,)*);
                    type RequestBody = #request_type;
                    type ResponseBody = #response_type;
                    type ErrorBody = #error_type;

                    const METADATA: #api_crate::OperationMetadata = #metadata;
                }
            }
        })
        .collect();
    let inventory: Vec<_> = api
        .operations
        .iter()
        .map(|operation| metadata_tokens(operation, &api_crate))
        .collect();

    Ok(quote! {
        #item

        #[doc = "Deterministic operation inventory for the API trait."]
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
        pub struct #metadata_ident;

        impl #api_crate::ApiContract for #metadata_ident {
            const OPERATIONS: &'static [#api_crate::OperationMetadata] = &[
                #(#inventory),*
            ];
        }

        #[doc = "Typed markers generated for this API trait's operations."]
        pub mod #module_ident {
            #(#marker_items)*
        }

        #(#operation_impls)*

        #reqwest_adapter
        #axum_adapter
    })
}

fn metadata_tokens(
    operation: &Operation,
    api_crate: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let operation_id = &operation.operation_id;
    let method = operation.method.tokens(api_crate);
    let path = &operation.path;
    let request_kind = if operation.request_body.is_some() {
        quote!(#api_crate::WireKind::Json)
    } else {
        quote!(#api_crate::WireKind::Empty)
    };
    let response_kind = if operation.response_body.is_some() {
        quote!(#api_crate::WireKind::Json)
    } else {
        quote!(#api_crate::WireKind::Empty)
    };
    let response_status = operation.response_status;
    let error = match operation.error_status {
        Some(status) => quote! {
            ::core::option::Option::Some(#api_crate::ResponseMetadata {
                status: #status,
                body: #api_crate::BodyMetadata {
                    wire_kind: #api_crate::WireKind::Json,
                },
            })
        },
        None => quote!(::core::option::Option::None),
    };
    let parameters = operation.parameters.iter().map(|parameter| {
        let rust_name = &parameter.rust_name;
        let wire_name = &parameter.wire_name;
        let location = parameter.location.tokens(api_crate);
        quote! {
            #api_crate::ParameterMetadata {
                rust_name: #rust_name,
                wire_name: #wire_name,
                location: #location,
            }
        }
    });

    quote! {
        #api_crate::OperationMetadata {
            operation_id: #operation_id,
            method: #method,
            path: #path,
            parameters: &[#(#parameters),*],
            request_body: #api_crate::BodyMetadata { wire_kind: #request_kind },
            response: #api_crate::ResponseMetadata {
                status: #response_status,
                body: #api_crate::BodyMetadata { wire_kind: #response_kind },
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

fn ident_text(ident: &Ident) -> String {
    let value = ident.to_string();
    value.strip_prefix("r#").unwrap_or(&value).to_owned()
}

fn operation_marker_ident(method: &Ident) -> Ident {
    let candidate = to_pascal_case(method);
    if let Ok(mut marker) = syn::parse_str::<Ident>(&candidate) {
        marker.set_span(method.span());
        return marker;
    }

    let encoded = ident_text(method)
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ident::new(&format!("Operation{encoded}"), method.span())
}

fn to_pascal_case(ident: &Ident) -> String {
    ident_text(ident)
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
    let value = ident_text(ident);
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
