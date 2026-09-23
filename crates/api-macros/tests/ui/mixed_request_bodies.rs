use api_macros::api;

pub struct JsonBody;
pub struct Response;

#[api]
pub trait MixedRequestBodies {
    #[post("/payload")]
    async fn upload(
        &self,
        #[body] json: JsonBody,
        #[binary] binary: api_macros::BinaryBody,
    ) -> Response;
}

fn main() {}
