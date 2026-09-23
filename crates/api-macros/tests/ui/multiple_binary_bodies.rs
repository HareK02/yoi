use api_macros::api;

pub struct Response;

#[api]
pub trait MultipleBinaryBodies {
    #[post("/payload")]
    async fn upload(
        &self,
        #[binary] first: api_macros::BinaryBody,
        #[binary] second: api_macros::BinaryBody,
    ) -> Response;
}

fn main() {}
