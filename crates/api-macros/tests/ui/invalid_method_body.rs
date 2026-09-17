use api_macros::api;

pub struct Request;
pub struct Response;

#[api]
pub trait InvalidMethodBody {
    #[get("/widgets")]
    async fn list(&self, #[body] request: Request) -> Response;
}

fn main() {}
