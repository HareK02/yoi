use api_macros::api;

pub struct Response;

#[api]
pub trait InvalidBinaryType {
    #[put("/payload")]
    async fn upload(&self, #[binary] body: Vec<u8>) -> Response;
}

fn main() {}
