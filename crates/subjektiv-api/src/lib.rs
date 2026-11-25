#![doc = include_str!("../README.md")]
mod chat;
mod chat_transport;
pub mod messaging;
pub mod worker_initialization;
pub use chat::*;
pub use chat_transport::*;
pub use messaging::*;

#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatCreateConversationRequest {}
#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatConversation {}
#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatUserMessageRequest {}
#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatSendMessageRequest {}
#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatMessagePageQuery {}
#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatMessagePage {}
#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatMessage {}
#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatReceiveResponse {}
#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatWorkerRequest {}
#[cfg(feature = "openapi")]
impl api_macros::openapi::OpenApiSchema for ChatWorkerResponse {}
