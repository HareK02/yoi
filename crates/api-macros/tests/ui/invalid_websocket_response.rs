use api_macros::api;

pub struct Frame;

#[api]
pub trait InvalidWebSocketResponseApi {
    #[websocket(
        "/events",
        method = GET,
        client_to_server = Frame,
        server_to_client = Frame,
        path_parameters = [],
        status = 101
    )]
    type Events;
}

fn main() {}
