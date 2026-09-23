use api_macros::api;

pub struct Body;

#[api]
pub trait UndeclaredStatusApi {
    #[get(
        "/status",
        responses = [(status = 200, body = Body)]
    )]
    async fn status(&self) -> undeclared_status_api_responses::Status;
}

fn invalid() -> undeclared_status_api_responses::Status {
    undeclared_status_api_responses::Status::Status201 { body: Body }
}

fn main() {}
