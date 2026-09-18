#![allow(async_fn_in_trait, private_interfaces)]

use api_macros::api;

struct PrivateRequest;
pub struct Response;

#[api]
pub trait PrivateBody {
    #[post("/widgets")]
    async fn create(&self, #[body] request: PrivateRequest) -> Response;
}

fn main() {}
