//! Built-in tools for the Yoi LLM agent.
//!
//! Read / Write / Edit / Glob / Grep / Bash operate through a host-owned
//! [`workdir::Workdir`] handle. This crate owns tool schemas, rendering, and
//! read-before-edit tracking; it does not own Workdir identity or lifecycle.
//!
//! Bash is intentionally not sandboxed. The Workdir supplies its initial cwd
//! and command capability, while the Runtime process and OS user remain the
//! trusted execution boundary.

pub mod error;
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
pub use tracker::Tracker;
pub use web::{web_fetch_tool, web_search_tool};
pub use write::write_tool;

/// Build the local filesystem/command tool surface implemented by a Workdir.
/// Profile/manifest policy may narrow this set further in the Engine.
pub fn core_builtin_tools(
    workdir: workdir::WorkdirHandle,
    tracker: Tracker,
    bash_output_dir: std::path::PathBuf,
) -> Vec<llm_engine::tool::ToolDefinition> {
    use workdir::WorkdirCapability;

    let capabilities = workdir.capabilities();
    let mut tools = Vec::with_capacity(6);
    if capabilities.supports(WorkdirCapability::Read) {
        tools.push(read_tool(workdir.clone(), tracker.clone()));
    }
    if capabilities.supports(WorkdirCapability::Write) {
        tools.push(write_tool(workdir.clone(), tracker.clone()));
    }
    if capabilities.supports(WorkdirCapability::Edit) {
        tools.push(edit_tool(workdir.clone(), tracker));
    }
    if capabilities.supports(WorkdirCapability::Glob) {
        tools.push(glob_tool(workdir.clone()));
    }
    if capabilities.supports(WorkdirCapability::Grep) {
        tools.push(grep_tool(workdir.clone()));
    }
    if capabilities.supports(WorkdirCapability::Command) {
        tools.push(bash_tool(workdir, bash_output_dir));
    }
    tools
}

pub fn web_builtin_tools(
    web_config: Option<manifest::WebConfig>,
) -> Vec<llm_engine::tool::ToolDefinition> {
    vec![
        web_search_tool(web::WebTools::new(web_config.clone())),
        web_fetch_tool(web::WebTools::new(web_config)),
    ]
}

#[cfg(test)]
mod workdir_tool_tests {
    use super::*;
    use manifest::{Scope, SharedScope};
    use std::sync::Arc;
    use tempfile::TempDir;
    use workdir::{LocalWorkdir, WorkdirCapabilities, WorkdirHandle};

    #[test]
    fn read_only_workdir_exposes_only_observation_tools() {
        let dir = TempDir::new().unwrap();
        let workdir: WorkdirHandle = Arc::new(LocalWorkdir::materialized(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            SharedScope::new(Scope::writable(dir.path()).unwrap()),
            WorkdirCapabilities::READ_ONLY,
        ));
        let names = core_builtin_tools(workdir, Tracker::new(), dir.path().join("output"))
            .into_iter()
            .map(|definition| definition().0.name)
            .collect::<Vec<_>>();

        assert_eq!(names, ["Read", "Glob", "Grep"]);
    }
}
