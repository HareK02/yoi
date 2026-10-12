//! Type states that restrict changes to Engine configuration.
//!
//! Transitioning from [`Mutable`] to [`Locked`] fixes the configuration and history
//! prefix length, restricting changes to system prompts and tool definitions during
//! execution. Changes require an explicit unlock because altering the prefix may
//! prevent prompt-cache reuse. History itself belongs to the caller: these states
//! neither prevent external history mutations nor guarantee cache hits.

/// Sealed trait for Engine type states.
///
/// External implementations are prohibited so the crate controls the separation
/// between configuration and execution APIs.
pub trait EngineState: private::Sealed + Send + Sync + 'static {}

mod private {
    pub trait Sealed {}
}

/// State in which configuration can be edited.
///
/// Transitions to [`Locked`] through `Engine::lock` or the first `run`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Mutable;

impl private::Sealed for Mutable {}
impl EngineState for Mutable {}

/// State with fixed configuration and history prefix length.
///
/// `Engine::unlock` returns to [`Mutable`] and permits configuration changes,
/// but requests made after those changes may no longer reuse the previous cache.
#[derive(Debug, Clone, Copy, Default)]
pub struct Locked;

impl private::Sealed for Locked {}
impl EngineState for Locked {}
