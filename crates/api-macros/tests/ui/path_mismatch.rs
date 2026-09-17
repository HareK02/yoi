use api_macros::api;

pub struct Response;

#[api]
pub trait PathMismatch {
    #[get("/widgets/{widget_id}")]
    async fn get(&self, #[path] other_id: u64) -> Response;
}

fn main() {}
