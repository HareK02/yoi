//! Built-in tools for the Insomnia LLM agent.
//!
//! Implements Read / Write / Edit / Glob / Grep on top of the `llm-worker`
//! `Tool` infrastructure. Filesystem access is mediated by two orthogonal
//! concerns:
//!
//! - [`ScopedFs`] — pod-lifetime, expresses the write-block boundary for
//!   the current scope. Derived from the manifest and shareable across
//!   sessions.
//! - [`Tracker`] — session-lifetime, enforces the "read before edit"
//!   policy via content hashes and tracks the recency of touched files.
//!   Recreated fresh per session.
//!
//! The Pod layer owns both instances and passes them to
//! [`builtin_tools`] when registering tools on a `Worker`.

pub mod error;
pub mod scoped_fs;
pub mod tracker;

mod edit;
mod glob;
mod grep;
mod read;
mod write;

pub use edit::edit_tool;
pub use error::ToolsError;
pub use glob::glob_tool;
pub use grep::grep_tool;
pub use read::read_tool;
pub use scoped_fs::ScopedFs;
pub use tracker::Tracker;
pub use write::write_tool;

/// Register all builtin tools, wiring them to a shared `ScopedFs`
/// (pod-lifetime) and `Tracker` (session-lifetime).
///
/// All returned factories share the same tracker instance so that
/// `Read` / `Write` / `Edit` see a consistent history across tool
/// invocations within a single session.
pub fn builtin_tools(
    fs: ScopedFs,
    tracker: Tracker,
) -> Vec<llm_worker::tool::ToolDefinition> {
    vec![
        read_tool(fs.clone(), tracker.clone()),
        write_tool(fs.clone(), tracker.clone()),
        edit_tool(fs.clone(), tracker.clone()),
        glob_tool(fs.clone()),
        grep_tool(fs),
    ]
}
