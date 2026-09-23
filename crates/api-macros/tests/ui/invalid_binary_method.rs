use api_macros::api;

pub struct Response;

#[api]
pub trait InvalidBinaryMethod {
    #[get("/payload")]
    async fn download(&self, #[binary] body: api_macros::BinaryBody) -> Response;
}

fn main() {}
