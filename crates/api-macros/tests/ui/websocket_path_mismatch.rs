use api_macros::api;

pub struct Frame;

#[api]
pub trait InvalidWebSocketPathApi {
    #[websocket(
        "/widgets/{widget_id}/events",
        method = GET,
        client_to_server = Frame,
        server_to_client = Frame,
        path_parameters = [other_id: u64]
    )]
    type Events;
}

fn main() {}
