use api_macros::api;

pub struct Body;

#[api]
pub trait InvalidResponseHeaderApi {
    #[get(
        "/invalid",
        responses = [(status = 200, body = Body, headers = [("bad header", String)])]
    )]
    async fn invalid(&self) -> invalid_response_header_api_responses::Invalid;
}

fn main() {}
