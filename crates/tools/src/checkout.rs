//! Native checkout entrypoint using the same processing as ordinary filesystem Tools.
//!
//! The Host binds alias, generation, path and validator from the checkout route; none
//! can be overridden by Operation arguments. File reads/mutations dispatch through
//! the provider's checked execution boundary. Search shares normal request and
//! rendering processing but dispatches through checkout_search so provider scope
//! layers and output rebasing are preserved; checks bracket the bound directory
//! observation, not an atomic snapshot of the searched tree.
use std::sync::Arc;

use agen::tool::{ToolError, ToolExecutionContext, ToolOutput};
use serde::de::DeserializeOwned;
use serde_json::Value;
use workdir::{WorkdirPath, WorkdirSessionCapability, WorkdirSessionRouter};

use crate::{ToolsError, Tracker, file_target::FileTarget};

pub struct CheckoutToolOutput {
    pub output: ToolOutput,
    pub paths: Vec<WorkdirPath>,
    pub validator: Option<Vec<u8>>,
}

/// Execute an exact named native Operation on a route-bound checkout target.
/// `Create.path` and search `path` are relative to the bound directory; returned
/// paths are typed checkout-root-relative provider paths, never parsed output.
#[allow(clippy::too_many_arguments)]
pub async fn execute_checkout_tool(
    router: Arc<WorkdirSessionRouter>,
    tracker: Tracker,
    alias: &str,
    generation: u64,
    path: WorkdirPath,
    validator: Vec<u8>,
    tool_name: &str,
    arguments: Value,
    ctx: ToolExecutionContext,
) -> Result<CheckoutToolOutput, ToolError> {
    let (capability, allowed): (_, &[&str]) = match tool_name {
        "Read" => (WorkdirSessionCapability::Read, &["offset", "limit"]),
        "Edit" => (
            WorkdirSessionCapability::Edit,
            &["old_string", "new_string", "replace_all"],
        ),
        "Write" => (WorkdirSessionCapability::Write, &["content"]),
        "Create" => (WorkdirSessionCapability::Write, &["path", "content"]),
        "Glob" => (WorkdirSessionCapability::Glob, &["pattern", "path"]),
        "Grep" => (
            WorkdirSessionCapability::Grep,
            &[
                "pattern",
                "path",
                "glob",
                "type",
                "case_insensitive",
                "-B",
                "-A",
                "-C",
                "multiline",
                "output_mode",
                "head_limit",
                "offset",
            ],
        ),
        _ => {
            return Err(ToolError::InvalidArgument(format!(
                "unknown checkout Operation: {tool_name}"
            )));
        }
    };
    let mut arguments = arguments
        .as_object()
        .cloned()
        .ok_or_else(|| ToolError::InvalidArgument("checkout arguments must be an object".into()))?;
    if let Some(key) = arguments
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(ToolError::InvalidArgument(format!(
            "unknown or route override checkout argument: {key}"
        )));
    }
    // Do not rebuild a singleton router: this is the original router identity and
    // tracker namespace shared with normal Tool calls and its detach/reuse fence.
    let selected = crate::routing::resolve_session(&router, Some(alias), capability)?;
    if selected.generation != generation {
        return Err(stale_checkout(
            "checkout attachment generation changed; resolve the checkout again",
        ));
    }
    // Checkout paths are always logical and cannot use Tool-mode absolute scope.
    let path = WorkdirPath::new(path.as_str()).map_err(ToolsError::from)?;
    let tracker = tracker.scoped_attachment(&selected.alias, selected.generation);
    let target = FileTarget {
        session: selected.session.clone(),
        path: path.clone(),
        validator: Some(validator.clone()),
    };
    match tool_name {
        "Read" | "Edit" | "Write" => {
            arguments.insert("file_path".into(), Value::String(path.to_string()));
            let arguments = Value::Object(arguments);
            match tool_name {
                "Read" => {
                    crate::read::execute_read(target, tracker, decode(arguments, tool_name)?).await
                }
                "Edit" => {
                    crate::edit::execute_edit(target, tracker, decode(arguments, tool_name)?, ctx)
                        .await
                }
                _ => {
                    let params: crate::write::WriteParams = decode(arguments, tool_name)?;
                    crate::write::execute_write(target, tracker, params.content, None, ctx).await
                }
            }
        }
        "Create" => {
            #[derive(serde::Deserialize)]
            struct CreateParams {
                path: String,
                content: String,
            }
            let params: CreateParams = decode(Value::Object(arguments), tool_name)?;
            let destination = relative_target(&path, &params.path, true)?;
            crate::write::execute_write(target, tracker, params.content, Some(destination), ctx)
                .await
        }
        "Glob" | "Grep" => {
            let before = selected
                .session
                .checkout_observe(path.clone())
                .await
                .map_err(crate::file_target::checked_error)?;
            if before.validator != validator {
                return Err(stale_search());
            }
            let mut result = if tool_name == "Glob" {
                let params: crate::glob::GlobParams = decode(Value::Object(arguments), tool_name)?;
                check_pattern(&params.pattern)?;
                let search_path =
                    relative_target(&path, params.path.as_deref().unwrap_or("."), false)?;
                crate::glob::execute_glob(
                    crate::search_target::SearchTarget::checkout(selected.session.clone()),
                    search_path,
                    params.pattern,
                )
                .await?
            } else {
                let params: crate::grep::GrepParams = decode(Value::Object(arguments), tool_name)?;
                if let Some(glob) = &params.glob {
                    check_pattern(glob)?;
                }
                let search_path =
                    relative_target(&path, params.path.as_deref().unwrap_or("."), false)?;
                crate::grep::execute_grep(
                    crate::search_target::SearchTarget::checkout(selected.session.clone()),
                    search_path,
                    params,
                )
                .await?
            };
            // Reject rather than emit a link/rendered path outside the binding.
            // Provider scope is still authoritative for both traversal and content.
            for result_path in &result.paths {
                let logical = WorkdirPath::new(result_path.as_str()).map_err(ToolsError::from)?;
                if !is_beneath(&path, &logical) {
                    return Err(crate::file_target::readonly_result_error(
                        "search paths escaped bound directory",
                    ));
                }
            }
            let after = selected
                .session
                .checkout_observe(path)
                .await
                .map_err(crate::file_target::checked_error)?;
            if after.validator != before.validator {
                return Err(stale_search());
            }
            result.validator = Some(after.validator);
            Ok(result)
        }
        _ => unreachable!("Operation names checked above"),
    }
}

fn decode<T: DeserializeOwned>(arguments: Value, name: &str) -> Result<T, ToolError> {
    serde_json::from_value(arguments)
        .map_err(|error| ToolError::InvalidArgument(format!("invalid {name} input: {error}")))
}

fn stale_search() -> ToolError {
    stale_checkout("checkout directory changed; resolve it again before searching")
}

pub(crate) fn stale_checkout(message: impl Into<String>) -> ToolError {
    ToolError::StructuredConflict {
        code: "checkout_stale".into(),
        message: message.into(),
    }
}

fn is_beneath(base: &WorkdirPath, path: &WorkdirPath) -> bool {
    base == &WorkdirPath::root()
        || std::path::Path::new(path.as_str()).starts_with(std::path::Path::new(base.as_str()))
}

fn relative_target(
    base: &WorkdirPath,
    relative: &str,
    strict: bool,
) -> Result<WorkdirPath, ToolError> {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.contains('\\')
        || relative.split('/').any(|part| part == "..")
    {
        return Err(ToolError::InvalidArgument(
            "path must be relative beneath the bound checkout directory, without parent traversal"
                .into(),
        ));
    }
    let relative = WorkdirPath::new(relative).map_err(ToolsError::from)?;
    let joined = if relative == WorkdirPath::root() {
        base.clone()
    } else if base == &WorkdirPath::root() {
        relative
    } else {
        WorkdirPath::new(&format!("{}/{}", base.as_str(), relative.as_str()))
            .map_err(ToolsError::from)?
    };
    if !is_beneath(base, &joined) || (strict && joined == *base) {
        return Err(ToolError::InvalidArgument(
            "destination must be strictly beneath the bound checkout directory".into(),
        ));
    }
    Ok(joined)
}

fn check_pattern(pattern: &str) -> Result<(), ToolError> {
    // Patterns filter descendants, not an alternate filesystem starting point.
    if pattern.starts_with('/') || pattern.split('/').any(|part| part == "..") {
        return Err(ToolError::InvalidArgument(
            "search glob must stay beneath the bound checkout directory".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "checkout_tests.rs"]
mod tests;
