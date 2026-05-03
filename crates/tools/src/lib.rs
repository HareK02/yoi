//! Built-in tools for the Insomnia LLM agent.
//!
//! Implements Read / Write / Edit / Glob / Grep / Bash on top of the
//! `llm-worker` `Tool` infrastructure. Filesystem access is mediated by
//! two orthogonal concerns:
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
//!
//! `Bash` is the lone exception — its child processes bypass `ScopedFs`
//! entirely. Safety for arbitrary command execution is delegated to the
//! Permission layer (deny/allow rules on the command string).

pub mod error;
pub mod scoped_fs;
pub mod task;
pub mod tracker;

mod bash;
mod edit;
mod glob;
mod grep;
mod read;
mod write;

pub use bash::bash_tool;
pub use edit::edit_tool;
pub use error::ToolsError;
pub use glob::glob_tool;
pub use grep::grep_tool;
pub use read::read_tool;
pub use scoped_fs::ScopedFs;
pub use task::{TaskEntry, TaskSnapshot, TaskStatus, TaskStore, task_tools};
pub use tracker::Tracker;
pub use write::write_tool;

/// Register all builtin tools, wiring them to a shared `ScopedFs`
/// (pod-lifetime) and `Tracker` (session-lifetime).
///
/// All returned factories share the same tracker instance so that
/// `Read` / `Write` / `Edit` see a consistent history across tool
/// invocations within a single session.
///
/// `bash_output_dir` is where the Bash tool spills long outputs. The
/// caller is responsible for adding that path to the readable scope
/// (see [`manifest::Scope::with_extra_read`]) so the agent can `Read`
/// the saved files.
pub fn builtin_tools(
    fs: ScopedFs,
    tracker: Tracker,
    task_store: TaskStore,
    bash_output_dir: std::path::PathBuf,
) -> Vec<llm_worker::tool::ToolDefinition> {
    let mut defs = vec![
        read_tool(fs.clone(), tracker.clone()),
        write_tool(fs.clone(), tracker.clone()),
        edit_tool(fs.clone(), tracker),
        glob_tool(fs.clone()),
        grep_tool(fs.clone()),
        bash_tool(fs, bash_output_dir),
    ];
    defs.extend(task_tools(task_store));
    defs
}
