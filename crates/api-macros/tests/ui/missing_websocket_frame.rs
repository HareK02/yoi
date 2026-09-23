use api_macros::api;

pub struct Frame;

#[api]
pub trait MissingWebSocketFrameApi {
    #[websocket(
        "/events",
        method = GET,
        client_to_server = Frame,
        path_parameters = []
    )]
    type Events;
}

fn main() {}
