//! Worker State
//!
//! State marker types for cache protection using the Type-state pattern.
//! Worker has state transitions from `Mutable` → `Locked`.

/// Marker trait representing Worker state
///
/// This trait is sealed and cannot be implemented externally.
pub trait WorkerState: private::Sealed + Send + Sync + 'static {}

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
/// Can transition to [`Locked`] state via `Worker::lock()`.
///
/// # Examples
///
/// ```ignore
/// use llm_worker::Worker;
///
/// let mut worker = Worker::new(client)
///     .system_prompt("You are helpful.");
///
/// // History can be edited
/// worker.push_message(Message::user("Hello"));
/// worker.clear_history();
///
/// // Lock to protected state
/// let locked = worker.lock();
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Mutable;

impl private::Sealed for Mutable {}
impl WorkerState for Mutable {}

/// Cache locked state (cache protected)
///
/// In this state, the following restrictions apply:
/// - System prompt cannot be changed
/// - Existing message history cannot be modified (only appending to the end)
///
/// To ensure LLM API KV cache hits,
/// using this state during execution is recommended.
///
/// Can return to [`Mutable`] state via `Worker::unlock()`,
/// but note that cache protection will be released.
#[derive(Debug, Clone, Copy, Default)]
pub struct Locked;

impl private::Sealed for Locked {}
impl WorkerState for Locked {}
