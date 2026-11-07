use api_macros::api;

pub struct Error;

#[api]
pub trait Wildcard {
    #[get("/wildcard", additional_error_content_types = ["application/*"])]
    async fn wildcard(&self) -> Result<(), Error>;
}

#[api]
pub trait DuplicateMedia {
    #[get("/duplicate", additional_error_content_types = ["application/problem+json", "APPLICATION/PROBLEM+JSON"])]
    async fn duplicate(&self) -> Result<(), Error>;
}

#[api]
pub trait DefaultMedia {
    #[get("/default", additional_error_content_types = ["application/json"])]
    async fn default_media(&self) -> Result<(), Error>;
}

#[api]
pub trait NoError {
    #[get("/infallible", additional_error_content_types = ["application/problem+json"])]
    async fn infallible(&self);
}

#[api]
pub trait EmptyMedia {
    #[get("/empty", additional_error_content_types = [])]
    async fn empty(&self) -> Result<(), Error>;
}

#[api]
pub trait DuplicateOption {
    #[get("/duplicate-option", additional_error_content_types = [], additional_error_content_types = [])]
    async fn duplicate_option(&self) -> Result<(), Error>;
}

#[api]
pub trait ParameterizedMedia {
    #[get("/parameterized", additional_error_content_types = ["application/problem+json; charset=utf-8"])]
    async fn parameterized(&self) -> Result<(), Error>;
}

fn main() {}
