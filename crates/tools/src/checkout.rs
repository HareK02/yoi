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
    /// Typed directory page, including its provider-coordinate continuation.
    /// Special entries are path coordinates, not checked checkout observations.
    /// The Host owns projection into canonical Worldspace entries and cursors.
    pub listing: Option<workdir::ListResult>,
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
    // Native file arguments come from the same definitions used by ordinary
    // Tools and both WIP projections. Route fields are intentionally not in them.
    let file_schema = fs_operation::text::TextOperation::from_name(&tool_name.to_lowercase(), true)
        .map(|operation| operation.argument_schema());
    let file_fields: Vec<&str> = file_schema
        .as_ref()
        .and_then(|schema| schema.as_value()["properties"].as_object())
        .map(|properties| properties.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let (capability, allowed): (_, &[&str]) = match tool_name {
        "Read" => (WorkdirSessionCapability::Read, &file_fields),
        "List" => (WorkdirSessionCapability::Read, &["limit", "after"]),
        "Edit" => (WorkdirSessionCapability::Edit, &file_fields),
        "Write" => (WorkdirSessionCapability::Write, &file_fields),
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
                    crate::write::execute_write(target, tracker, params.write.content, None, ctx)
                        .await
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
        "List" | "Glob" | "Grep" => {
            let before = selected
                .session
                .checkout_observe(path.clone())
                .await
                .map_err(crate::file_target::checked_error)?;
            if before.validator != validator {
                return Err(stale_search());
            }
            let mut result = if tool_name == "List" {
                let params: ListParams = decode(Value::Object(arguments), tool_name)?;
                execute_list(selected.session.clone(), path.clone(), params).await?
            } else if tool_name == "Glob" {
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

/// Native List has no path override. `after.path` is already decoded by the Host
/// into checkout-root-relative provider coordinates, not directory-relative.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default = "default_list_limit")]
    limit: usize,
    #[serde(default)]
    after: Option<workdir::ListCursor>,
}

fn default_list_limit() -> usize {
    100
}

async fn execute_list(
    session: workdir::WorkdirSessionHandle,
    path: WorkdirPath,
    params: ListParams,
) -> Result<CheckoutToolOutput, ToolError> {
    if !(1..=1000).contains(&params.limit) {
        return Err(ToolError::InvalidArgument(
            "List limit must be between 1 and 1000".into(),
        ));
    }
    if let Some(after) = &params.after {
        if !is_direct_child(&path, &after.path) {
            return Err(ToolError::InvalidArgument(
                "List after must name a direct child of the bound directory".into(),
            ));
        }
    }
    let result = session
        .checkout_search(workdir::CheckoutSearchRequest::new(
            workdir::CheckoutSearchOperation::List(workdir::ListRequest {
                path: path.clone(),
                limit: params.limit,
                after: params.after,
            }),
        ))
        .await
        .map_err(crate::file_target::checked_error)?;
    let listing = match result {
        workdir::CheckoutSearchResult::List(listing) => listing,
        _ => return Err(crate::file_target::readonly_result_error("List")),
    };
    // Fail closed before the Host can publish typed links to provider results.
    if listing.entries.len() > params.limit
        || listing
            .entries
            .iter()
            .any(|entry| !is_direct_child(&path, &entry.path))
        || listing
            .next_after
            .as_ref()
            .is_some_and(|after| !is_direct_child(&path, &after.path))
    {
        return Err(crate::file_target::readonly_result_error(
            "List page escaped bound directory or exceeded its limit",
        ));
    }
    // The provider ordering key is directory/non-directory group plus path.
    // Preserve the typed kind; special entries remain continuable without
    // acquiring a File/Directory-only checkout observation for them.
    let valid_continuation = match (&listing.next_after, listing.entries.last()) {
        (Some(after), Some(last)) if listing.truncated => {
            after.path == last.path
                && (after.kind == workdir::EntryKind::Directory)
                    == (last.kind == workdir::EntryKind::Directory)
        }
        (None, _) => !listing.truncated,
        _ => false,
    };
    if !valid_continuation {
        return Err(crate::file_target::readonly_result_error(
            "List continuation does not match the truncated page's last ordering key",
        ));
    }
    let summary = format!(
        "Listed {} of {} entries in {}{}",
        listing.entries.len(),
        listing.total_entries,
        path,
        if listing.truncated {
            " (truncated)"
        } else {
            ""
        },
    );
    Ok(CheckoutToolOutput {
        output: ToolOutput {
            summary,
            content: None,
            attachments: Vec::new(),
        },
        paths: Vec::new(),
        listing: Some(listing),
        validator: None,
    })
}

fn is_direct_child(base: &WorkdirPath, path: &WorkdirPath) -> bool {
    let Ok(logical) = WorkdirPath::new(path.as_str()) else {
        return false;
    };
    let parent = std::path::Path::new(logical.as_str()).parent();
    let expected = if base == &WorkdirPath::root() {
        std::path::Path::new("")
    } else {
        std::path::Path::new(base.as_str())
    };
    logical != WorkdirPath::root() && parent == Some(expected)
}

fn decode<T: DeserializeOwned>(arguments: Value, name: &str) -> Result<T, ToolError> {
    crate::error::decode_file_value(arguments, name)
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
