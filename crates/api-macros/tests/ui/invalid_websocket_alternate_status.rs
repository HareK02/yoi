use api_macros::api;

pub struct Frame;

#[api]
pub trait InvalidWebSocketAlternateStatusApi {
    #[websocket(
        "/events",
        method = GET,
        client_to_server = Frame,
        server_to_client = Frame,
        path_parameters = [],
        alternate_status = 204
    )]
    type Events;
}

fn main() {}
