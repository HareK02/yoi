use api_macros::api;

pub struct Error;

#[api]
pub trait NoReqwest {
    #[get("/error", client_error_hook = true)]
    async fn error(&self) -> Result<(), Error>;
}

#[api(reqwest)]
pub trait NoError {
    #[post("/infallible", client_error_hook = true)]
    async fn infallible(&self);
}

#[api(reqwest)]
pub trait DuplicateHook {
    #[get("/duplicate", client_error_hook = true, client_error_hook = false)]
    async fn duplicate(&self) -> Result<(), Error>;
}

fn main() {}
