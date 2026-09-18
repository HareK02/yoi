use api_macros::api;

pub struct Response;
pub struct PublicError;

#[api]
pub trait InvalidErrorStatus {
    #[get("/widgets", error_status = 200)]
    async fn list(&self) -> Result<Response, PublicError>;
}

fn main() {}
