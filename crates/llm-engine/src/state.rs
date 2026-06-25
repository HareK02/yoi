//! Engine State
//!
//! State marker types for cache protection using the Type-state pattern.
//! Engine has state transitions from `Mutable` → `Locked`.

/// Marker trait representing Engine state
///
/// This trait is sealed and cannot be implemented externally.
pub trait EngineState: private::Sealed + Send + Sync + 'static {}

mod private {
    pub trait Sealed {}
}

/// Mutable state (editable)
///
/// In this state, the following operations are available:
/// - Setting/changing system prompt
/// - Editing message history (add, delete, clear)
/// - Registering tools and hooks
///
/// Can transition to [`Locked`] state via `Engine::lock()`.
///
/// # Examples
///
/// ```ignore
/// use llm_engine::Engine;
///
/// let mut engine = Engine::new(client)
///     .system_prompt("You are helpful.");
///
/// // History can be edited
/// engine.push_message(Message::user("Hello"));
/// engine.clear_history();
///
/// // Lock to protected state
/// let locked = engine.lock();
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Mutable;

impl private::Sealed for Mutable {}
impl EngineState for Mutable {}

/// Cache locked state (cache protected)
///
/// In this state, the following restrictions apply:
/// - System prompt cannot be changed
/// - Existing message history cannot be modified (only appending to the end)
///
/// To ensure LLM API KV cache hits,
/// using this state during execution is recommended.
///
/// Can return to [`Mutable`] state via `Engine::unlock()`,
/// but note that cache protection will be released.
#[derive(Debug, Clone, Copy, Default)]
pub struct Locked;

impl private::Sealed for Locked {}
impl EngineState for Locked {}
