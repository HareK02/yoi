use api_macros::api;

pub struct Frame;
pub struct Response;

#[api]
pub trait InvalidWebSocketResponseBodyApi {
    #[websocket(
        "/events",
        method = GET,
        client_to_server = Frame,
        server_to_client = Frame,
        path_parameters = [],
        response = Response
    )]
    type Events;
}

fn main() {}
