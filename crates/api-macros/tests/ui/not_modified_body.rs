use api_macros::api;

pub struct Body;

#[api]
pub trait NotModifiedBodyApi {
    #[get(
        "/not-modified",
        responses = [(status = 304, body = Body)]
    )]
    async fn not_modified(&self) -> not_modified_body_api_responses::NotModified;
}

fn main() {}
