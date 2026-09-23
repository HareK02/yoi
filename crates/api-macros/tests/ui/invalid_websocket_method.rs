use api_macros::api;

pub struct Frame;

#[api]
pub trait InvalidWebSocketMethodApi {
    #[websocket(
        "/events",
        method = POST,
        client_to_server = Frame,
        server_to_client = Frame,
        path_parameters = []
    )]
    type Events;
}

fn main() {}
