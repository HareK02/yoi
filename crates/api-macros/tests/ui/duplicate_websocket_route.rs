use api_macros::api;

pub struct Frame;
pub struct Response;

#[api]
pub trait DuplicateTransportRouteApi {
    #[get("/widgets/{widget_id}/events")]
    async fn events(&self, #[path] widget_id: u64) -> Response;

    #[websocket(
        "/widgets/{widget_id}/events",
        method = GET,
        client_to_server = Frame,
        server_to_client = Frame,
        path_parameters = [widget_id: u64]
    )]
    type WidgetEvents;
}

fn main() {}
