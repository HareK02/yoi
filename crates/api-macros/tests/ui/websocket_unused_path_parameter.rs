use api_macros::api;

pub struct Frame;

#[api]
pub trait UnusedWebSocketPathParameterApi {
    #[websocket(
        "/events",
        method = GET,
        client_to_server = Frame,
        server_to_client = Frame,
        path_parameters = [unused: u64]
    )]
    type Events;
}

fn main() {}
