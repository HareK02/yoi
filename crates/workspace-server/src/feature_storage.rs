//! Compatibility boundary re-exporting shared Host feature storage management to the Backend.
//!
//! The feature-storage crate owns storage registration and migrations. This module adds neither
//! a Backend-specific storage lifecycle nor a separate schema authority.
pub use feature_storage::*;
