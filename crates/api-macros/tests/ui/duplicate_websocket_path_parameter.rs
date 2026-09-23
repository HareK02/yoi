use api_macros::api;

pub struct Frame;

#[api]
pub trait DuplicateWebSocketPathParameterApi {
    #[websocket(
        "/widgets/{widget_id}/events",
        method = GET,
        client_to_server = Frame,
        server_to_client = Frame,
        path_parameters = [widget_id: u64, widget_id: String]
    )]
    type Events;
}

fn main() {}
