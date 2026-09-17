use api_macros::api;

pub struct Response;

#[api]
pub trait MarkerCollision {
    #[get("/first")]
    async fn foo_bar(&self) -> Response;

    #[get("/second")]
    async fn foo__bar(&self) -> Response;
}

fn main() {}
