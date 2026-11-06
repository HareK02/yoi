use api_macros::api;

#[api(client_error_hook)]
pub trait Invalid {}

fn main() {}
