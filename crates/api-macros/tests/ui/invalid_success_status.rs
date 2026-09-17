use api_macros::api;

pub struct Response;

#[api]
pub trait InvalidSuccessStatus {
    #[get("/widgets", status = 404)]
    async fn list(&self) -> Response;
}

fn main() {}
