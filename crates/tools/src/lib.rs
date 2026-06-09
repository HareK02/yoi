//! Built-in tools for the Yoi LLM agent.
//!
//! Implements Read / Write / Edit / Glob / Grep / Bash on top of the
//! `llm-worker` `Tool` infrastructure. Filesystem access is mediated by
//! two orthogonal concerns:
//!
//! - [`ScopedFs`] — Pod-process lifetime, expresses the write-block
//!   boundary for the current scope. Derived from the manifest; not
//!   persisted across Pod restart.
//! - [`Tracker`] — Pod-process lifetime, enforces the "read before edit"
//!   policy via content hashes and tracks the recency of touched files.
//!   Recreated fresh on each Pod start (including resume).
//!
//! The Pod layer owns both instances and passes them to
//! [`core_builtin_tools`] when registering tools on a `Worker`.
//!
//! `Bash` is the lone exception — its child processes bypass `ScopedFs`
//! entirely. Safety for arbitrary command execution is delegated to the
//! Permission layer (deny/allow rules on the command string).

pub mod error;
pub mod scoped_fs;
pub mod tracker;

mod bash;
mod edit;
mod glob;
mod grep;
mod read;
mod web;
mod write;

pub use bash::bash_tool;
pub use edit::edit_tool;
pub use error::ToolsError;
pub use glob::glob_tool;
pub use grep::grep_tool;
pub use read::read_tool;
pub use scoped_fs::ScopedFs;
pub use tracker::Tracker;
pub use web::{web_fetch_tool, web_search_tool};
pub use write::write_tool;

/// Register core builtin tools that do not require Pod-local task state,
/// wiring them to a shared `ScopedFs` (Pod-process lifetime) and `Tracker`
/// (Pod-process lifetime).
///
/// All returned factories share the same tracker instance so that
/// `Read` / `Write` / `Edit` see a consistent history across tool
/// invocations within a single Pod run.
///
/// `bash_output_dir` is where the Bash tool spills long outputs. The
/// caller is responsible for adding that path to the readable scope
/// (see [`manifest::Scope::with_extra_read`]) so the agent can `Read`
/// the saved files.
pub fn core_builtin_tools(
    fs: ScopedFs,
    tracker: Tracker,
    bash_output_dir: std::path::PathBuf,
) -> Vec<llm_worker::tool::ToolDefinition> {
    vec![
        read_tool(fs.clone(), tracker.clone()),
        write_tool(fs.clone(), tracker.clone()),
        edit_tool(fs.clone(), tracker),
        glob_tool(fs.clone()),
        grep_tool(fs.clone()),
        bash_tool(fs, bash_output_dir),
    ]
}

pub fn web_builtin_tools(
    web_config: Option<manifest::WebConfig>,
) -> Vec<llm_worker::tool::ToolDefinition> {
    vec![
        web_search_tool(web::WebTools::new(web_config.clone())),
        web_fetch_tool(web::WebTools::new(web_config)),
    ]
}
