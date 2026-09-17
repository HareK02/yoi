#![allow(async_fn_in_trait)]

use api_macros::{ApiContract, HttpMethod, NoBody, Operation, ParameterLocation, WireKind, api};

pub struct CreateWidget;
pub struct Widget;
pub struct Lookup;
pub struct PublicError;
pub struct SearchResult<T>(std::marker::PhantomData<T>);

#[api]
pub trait WidgetApi {
    #[post(
        "/widgets",
        operation_id = "widgets.create",
        status = 201,
        error_status = 422
    )]
    async fn create(
        &self,
        #[body] request: CreateWidget,
        #[header("x-request-id")] request_id: String,
    ) -> Result<Widget, PublicError>;

    #[get("/widgets/{widget_id}", operation_id = "widgets.get")]
    async fn lookup(&self, widget_id: u64, #[query] query: Lookup) -> Result<Widget, PublicError>;

    #[delete("/widgets/{widget_id}", operation_id = "widgets.delete")]
    async fn delete(&self, #[path] widget_id: u64) -> ();
}

#[api]
pub trait WrapperApi {
    #[get("/search", operation_id = "search")]
    async fn search(&self) -> SearchResult<Widget>;
}

#[api]
pub trait RawIdentifierApi {
    #[get("/type", operation_id = "raw.type")]
    async fn r#type(&self) -> Widget;
}

fn assert_operation_types<O>()
where
    O: Operation<
            Parameters = (CreateWidget, String),
            RequestBody = CreateWidget,
            ResponseBody = Widget,
            ErrorBody = PublicError,
        >,
{
}

#[test]
fn expansion_exposes_deterministic_metadata_and_type_connections() {
    assert_operation_types::<widget_api_operations::Create>();

    fn assert_named_wrapper<O: Operation<ResponseBody = SearchResult<Widget>>>() {}
    assert_named_wrapper::<wrapper_api_operations::Search>();
    let raw = <raw_identifier_api_operations::Type as Operation>::METADATA;
    assert_eq!(raw.operation_id, "raw.type");

    let operations = <WidgetApiMetadata as ApiContract>::OPERATIONS;
    assert_eq!(operations.len(), 3);
    assert_eq!(operations[0].operation_id, "widgets.create");
    assert_eq!(operations[1].operation_id, "widgets.delete");
    assert_eq!(operations[2].operation_id, "widgets.get");

    let create = <widget_api_operations::Create as Operation>::METADATA;
    assert_eq!(operations[0], create);
    assert_eq!(create.method, HttpMethod::Post);
    assert_eq!(create.path, "/widgets");
    assert_eq!(create.request_body.wire_kind, WireKind::Json);
    assert_eq!(create.response.status, 201);
    assert_eq!(create.response.body.wire_kind, WireKind::Json);
    assert_eq!(create.error_response.unwrap().status, 422);
    assert_eq!(create.parameters[0].location, ParameterLocation::Body);
    assert_eq!(create.parameters[1].location, ParameterLocation::Header);
    assert_eq!(create.parameters[1].wire_name, "x-request-id");

    let lookup = <widget_api_operations::Lookup as Operation>::METADATA;
    assert_eq!(lookup.parameters[0].location, ParameterLocation::Path);
    assert_eq!(lookup.parameters[1].location, ParameterLocation::Query);

    let delete = <widget_api_operations::Delete as Operation>::METADATA;
    assert_eq!(delete.response.status, 204);
    assert_eq!(delete.response.body.wire_kind, WireKind::Empty);
    assert_eq!(delete.error_response, None);

    fn assert_empty_request<O: Operation<RequestBody = NoBody>>() {}
    fn assert_empty_response<O: Operation<ResponseBody = NoBody, ErrorBody = NoBody>>() {}
    assert_empty_request::<widget_api_operations::Lookup>();
    assert_empty_response::<widget_api_operations::Delete>();
}
