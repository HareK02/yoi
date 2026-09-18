use api_macros::api;

pub struct FirstBody;
pub struct SecondBody;
pub struct Response;

#[api]
pub trait MultipleBodies {
    #[post("/widgets")]
    async fn create(
        &self,
        #[body] first: FirstBody,
        #[body] second: SecondBody,
    ) -> Response;
}

fn main() {}
