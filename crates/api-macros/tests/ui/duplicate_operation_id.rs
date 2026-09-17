use api_macros::api;

pub struct Response;

#[api]
pub trait DuplicateOperationId {
    #[get("/one", operation_id = "duplicate")]
    async fn one(&self) -> Response;

    #[get("/two", operation_id = "duplicate")]
    async fn two(&self) -> Response;
}

fn main() {}
