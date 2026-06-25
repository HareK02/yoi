//! Embedded Runtime domain API for Worker management.
//!
//! `worker-runtime` intentionally stays independent from HTTP/WebSocket servers,
//! filesystem persistence, provider execution, and the existing Worker host.  It
//! defines the in-process Runtime authority surface that higher layers can later
//! adapt into registries or web APIs.

pub mod catalog;
pub mod diagnostics;
pub mod error;
pub mod identity;
pub mod interaction;
pub mod management;
pub mod observation;
mod runtime;

pub use runtime::Runtime;
