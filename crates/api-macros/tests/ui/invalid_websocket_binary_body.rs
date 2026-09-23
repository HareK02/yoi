use api_macros::api;

pub struct Frame;
pub struct Body;

#[api]
pub trait InvalidWebSocketBinaryBodyApi {
    #[websocket(
        "/events",
        method = GET,
        client_to_server = Frame,
        server_to_client = Frame,
        path_parameters = [],
        binary = Body
    )]
    type Events;
}

fn main() {}
