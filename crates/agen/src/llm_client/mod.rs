//! Boundary between the Engine and provider communication.
//!
//! [`LlmClient`] accepts a shared [`Request`] and returns normalized [`Event`]s.
//! Provider-specific bodies, authentication requirements, and stream parsing are
//! separated into [`scheme`] and [`transport`] to limit how API changes affect
//! turn progression and tool execution semantics.
//!
//! Scheme capabilities describe what the wire format can safely handle.
//! The client construction layer owns per-model capability tables and alias
//! resolution; the Engine does not depend on that selection process.

pub mod auth;
pub mod capability;
pub mod client;
pub mod error;
pub mod event;
pub mod types;

pub mod retry;
pub mod scheme;
pub mod transport;

pub use auth::*;
pub use capability::*;
pub use client::*;
pub use error::*;
pub use event::*;
pub use types::*;
