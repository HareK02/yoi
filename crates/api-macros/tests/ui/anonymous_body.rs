use api_macros::api;

pub struct Response;

#[api]
pub trait AnonymousBody {
    #[post("/widgets")]
    async fn create(&self, #[body] request: (u64, u64)) -> Response;
}

fn main() {}
