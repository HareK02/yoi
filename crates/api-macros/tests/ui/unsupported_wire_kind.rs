use api_macros::api;

pub struct StreamRequest;
pub struct Response;

#[api]
pub trait UnsupportedWireKind {
    #[post("/widgets")]
    async fn create(&self, #[stream] request: StreamRequest) -> Response;
}

fn main() {}
