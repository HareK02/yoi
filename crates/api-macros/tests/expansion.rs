#![allow(async_fn_in_trait)]

use api_macros::{
    ApiContract, BinaryBody, HttpMethod, NoBody, Operation, ParameterLocation, TransportMetadata,
    WebSocketOperation, WireKind, api,
};

pub struct CreateWidget;
pub struct Widget;
pub struct Lookup;
pub struct PublicError;
pub struct SearchResult<T>(std::marker::PhantomData<T>);
pub struct ClientFrame;
pub struct ServerFrame;

#[api]
pub trait MixedTransportApi {
    #[get("/status")]
    async fn status(&self) -> Widget;

    #[websocket(
        "/widgets/{widget_id}/events",
        operation_id = "widgets.events",
        method = GET,
        client_to_server = ClientFrame,
        server_to_client = ServerFrame,
        path_parameters = [widget_id: u64]
    )]
    type WidgetEvents;
}

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
pub trait BinaryApi {
    #[put("/payload", operation_id = "payload.upload")]
    async fn upload(&self, #[binary] payload: BinaryBody) -> Widget;
}

#[api]
pub trait ConditionalApi {
    #[get(
        "/conditional",
        operation_id = "conditional.get",
        responses = [
            (status = 200, body = Widget, headers = [("etag", String), ("cache-control", String)]),
            (status = 304, headers = [("etag", String), ("cache-control", String)])
        ]
    )]
    async fn fetch(&self) -> conditional_api_responses::Fetch;
}

#[api]
pub trait RawIdentifierApi {
    #[get("/type", operation_id = "raw.type")]
    async fn r#type(&self) -> Widget;
}

#[api]
pub trait UnusualIdentifierApi {
    #[get("/underscores", operation_id = "unusual.underscores")]
    async fn __(&self) -> Widget;

    #[get("/underscore-digit", operation_id = "unusual.underscore_digit")]
    async fn _0(&self) -> Widget;
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
    let underscores = <unusual_identifier_api_operations::Operation5f5f as Operation>::METADATA;
    assert_eq!(underscores.operation_id, "unusual.underscores");
    let underscore_digit =
        <unusual_identifier_api_operations::Operation5f30 as Operation>::METADATA;
    assert_eq!(underscore_digit.operation_id, "unusual.underscore_digit");

    let operations = <WidgetApiMetadata as ApiContract>::OPERATIONS;
    assert_eq!(operations.len(), 3);
    assert_eq!(operations[0].operation_id, "widgets.create");
    assert_eq!(operations[1].operation_id, "widgets.delete");
    assert_eq!(operations[2].operation_id, "widgets.get");

    let create = <widget_api_operations::Create as Operation>::METADATA;
    assert_eq!(operations[0], create);
    assert_eq!(create.method, HttpMethod::Post);
    assert_eq!(create.path, "/widgets");
    assert_eq!(create.transport, TransportMetadata::Http);
    assert_eq!(create.request_body.wire_kind, WireKind::Json);
    assert_eq!(create.success_responses[0].status, 201);
    assert_eq!(create.success_responses[0].body.wire_kind, WireKind::Json);
    assert_eq!(create.error_responses[0].status, 422);
    assert_eq!(create.parameters[0].location, ParameterLocation::Body);
    assert_eq!(create.parameters[0].rust_type, "CreateWidget");
    assert_eq!(create.parameters[1].location, ParameterLocation::Header);
    assert_eq!(create.parameters[1].wire_name, "x-request-id");

    let lookup = <widget_api_operations::Lookup as Operation>::METADATA;
    assert_eq!(lookup.parameters[0].location, ParameterLocation::Path);
    assert_eq!(lookup.parameters[1].location, ParameterLocation::Query);

    let upload = <binary_api_operations::Upload as Operation>::METADATA;
    assert_eq!(upload.request_body.wire_kind, WireKind::Binary);
    assert_eq!(upload.parameters[0].location, ParameterLocation::Body);
    fn assert_binary_request<O: Operation<RequestBody = BinaryBody>>() {}
    assert_binary_request::<binary_api_operations::Upload>();

    let conditional = <conditional_api_operations::Fetch as Operation>::METADATA;
    assert_eq!(conditional.success_responses.len(), 2);
    assert_eq!(conditional.success_responses[0].status, 200);
    assert_eq!(
        conditional.success_responses[0].body.wire_kind,
        WireKind::Json
    );
    assert_eq!(conditional.success_responses[0].headers.len(), 2);
    assert_eq!(
        conditional.success_responses[0].headers[0].wire_name,
        "etag"
    );
    assert_eq!(
        conditional.success_responses[0].headers[0].rust_type,
        "String"
    );
    assert_eq!(conditional.success_responses[1].status, 304);
    assert_eq!(
        conditional.success_responses[1].body.wire_kind,
        WireKind::Empty
    );
    fn assert_conditional_response<
        O: Operation<ResponseBody = conditional_api_responses::Fetch>,
    >() {
    }
    assert_conditional_response::<conditional_api_operations::Fetch>();

    let delete = <widget_api_operations::Delete as Operation>::METADATA;
    assert_eq!(delete.success_responses[0].status, 204);
    assert_eq!(delete.success_responses[0].body.wire_kind, WireKind::Empty);
    assert!(delete.error_responses.is_empty());

    fn assert_empty_request<O: Operation<RequestBody = NoBody>>() {}
    fn assert_empty_response<O: Operation<ResponseBody = NoBody, ErrorBody = NoBody>>() {}
    assert_empty_request::<widget_api_operations::Lookup>();
    assert_empty_response::<widget_api_operations::Delete>();

    fn assert_websocket_types<
        O: WebSocketOperation<
                PathParameters = (u64,),
                ClientToServerFrame = ClientFrame,
                ServerToClientFrame = ServerFrame,
            >,
    >() {
    }
    assert_websocket_types::<mixed_transport_api_operations::WidgetEvents>();
    let websocket = <mixed_transport_api_operations::WidgetEvents as WebSocketOperation>::METADATA;
    assert_eq!(websocket.method, HttpMethod::Get);
    assert_eq!(websocket.path, "/widgets/{widget_id}/events");
    assert_eq!(websocket.parameters[0].rust_name, "widget_id");
    assert_eq!(websocket.parameters[0].rust_type, "u64");
    assert!(websocket.success_responses.is_empty());
    assert!(websocket.error_responses.is_empty());
    assert_eq!(
        websocket.transport,
        TransportMetadata::WebSocket {
            client_to_server_frame: "ClientFrame",
            server_to_client_frame: "ServerFrame",
        }
    );
}
