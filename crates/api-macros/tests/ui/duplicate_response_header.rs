use api_macros::api;

pub struct Body;

#[api]
pub trait DuplicateResponseHeaderApi {
    #[get(
        "/duplicate",
        responses = [(status = 200, body = Body, headers = [("etag", String), ("ETag", String)])]
    )]
    async fn duplicate(&self) -> duplicate_response_header_api_responses::Duplicate;
}

fn main() {}
