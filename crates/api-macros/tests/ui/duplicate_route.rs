use api_macros::api;

pub struct Response;

#[api]
pub trait DuplicateRoute {
    #[get("/widgets", operation_id = "widgets.first")]
    async fn first(&self) -> Response;

    #[get("/widgets", operation_id = "widgets.second")]
    async fn second(&self) -> Response;
}

fn main() {}
