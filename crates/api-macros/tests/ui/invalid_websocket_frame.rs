use api_macros::api;

pub struct Frame;

#[api]
pub trait InvalidWebSocketFrameApi {
    #[websocket(
        "/events",
        method = GET,
        client_to_server = (Frame, Frame),
        server_to_client = Frame,
        path_parameters = []
    )]
    type Events;
}

fn main() {}
