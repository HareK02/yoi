//! One-time Runtime schema-21 retirement of inert Workspace Memory policy.
//!
//! Only the mutable execution snapshot is changed, under Runtime's owner-locked
//! staging publication. Requests, configuration archives, fingerprints and receipts
//! are historical evidence and are never searched or rewritten here. Enabled or
//! ambiguous policy remains unreadable by the current Manifest decoder.

use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Retirement {
    Unchanged,
    Disabled,
    Unavailable(&'static str),
}

pub(super) fn legacy_profile(profile: &ProfileSelector) -> bool {
    match profile {
        ProfileSelector::Named(name) | ProfileSelector::Builtin(name) => {
            matches!(
                name.as_str(),
                "memory-consolidation" | "builtin:memory-consolidation"
            )
        }
    }
}

fn saved_manifest(snapshot: &serde_json::Value) -> &serde_json::Value {
    snapshot.get("manifest").unwrap_or(snapshot)
}

fn saved_legacy_profile(manifest: &serde_json::Value) -> bool {
    manifest
        .pointer("/profile/source/name")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|name| {
            matches!(
                name,
                "memory-consolidation" | "builtin:memory-consolidation"
            )
        })
}

// Missing enablement is not evidence of disablement. The nested form is the
// released resolved Manifest representation; the flat form predates that split.
// Inert nested extraction defaults do not activate a disabled enclosing Feature.
pub(super) fn retire_disabled_memory(snapshot: &mut serde_json::Value) -> Retirement {
    let manifest = saved_manifest(snapshot);
    if saved_legacy_profile(manifest) {
        return Retirement::Unavailable("removed Workspace Memory profile");
    }
    let Some(memory) = manifest.pointer("/feature/memory") else {
        return Retirement::Unchanged;
    };
    let known_keys = |value: &serde_json::Value, allowed: &[&str]| {
        value
            .as_object()
            .is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
    };
    let nested = memory.get("profile").is_some();
    if (nested
        && (!known_keys(memory, &["profile", "workspace_settings"])
            || !known_keys(
                &memory["profile"],
                &[
                    "enabled",
                    "staging_tools",
                    "resident",
                    "extraction",
                    "consolidation",
                ],
            )))
        || (!nested && !known_keys(memory, &["enabled", "staging"]))
    {
        return Retirement::Unavailable("unrecognized removed Workspace Memory policy");
    }
    let enabled = if memory.get("profile").is_some() {
        memory.pointer("/profile/enabled")
    } else {
        memory.get("enabled")
    }
    .and_then(serde_json::Value::as_bool);
    if enabled != Some(false) {
        return Retirement::Unavailable("enabled or ambiguous removed Workspace Memory policy");
    }
    // A disabled Feature with a bound old Workspace snapshot is contradictory,
    // not permission to discard authority-bearing data on a guessed reading.
    if memory
        .get("workspace_settings")
        .is_some_and(|settings| !settings.is_null())
    {
        return Retirement::Unavailable(
            "disabled Workspace Memory policy has bound Workspace settings",
        );
    }
    let manifest = if snapshot.get("manifest").is_some() {
        snapshot.get_mut("manifest").expect("checked envelope")
    } else {
        snapshot
    };
    manifest
        .get_mut("feature")
        .and_then(serde_json::Value::as_object_mut)
        .expect("memory was a child of feature")
        .remove("memory");
    // Pre-split snapshots placed legacy settings at the root. Remove these only
    // after the enclosing Workspace Feature was explicitly proven disabled.
    manifest
        .as_object_mut()
        .expect("manifest owns feature")
        .remove("memory");
    Retirement::Disabled
}

pub(super) fn unavailable_reason(
    metadata: &serde_json::Value,
    profile: &ProfileSelector,
) -> Option<&'static str> {
    if legacy_profile(profile) {
        return Some("removed Workspace Memory profile");
    }
    let mut snapshot = metadata.get("resolved_manifest_snapshot")?.clone();
    match retire_disabled_memory(&mut snapshot) {
        Retirement::Unavailable(reason) => Some(reason),
        _ => None,
    }
}

pub(super) fn record_unavailable_workers(
    document: &mut serde_json::Value,
    workers: Vec<(WorkerRef, &'static str)>,
    path: &Path,
) -> Result<(), RuntimeError> {
    for (worker_ref, reason) in workers {
        let id = document["next_diagnostic_id"].as_u64().ok_or_else(|| {
            runtime_store_corrupt(path, "Runtime diagnostic identity is missing".into())
        })?;
        let next = id.checked_add(1).ok_or_else(|| {
            runtime_store_corrupt(path, "Runtime diagnostic identity exhausted".into())
        })?;
        let diagnostic = RuntimeDiagnostic {
            id,
            worker_ref: Some(worker_ref),
            severity: DiagnosticSeverity::Warning,
            code: "workspace_memory_worker_retired".into(),
            message: format!(
                "{reason}; saved execution is unavailable; create a replacement with current policy"
            ),
        };
        document["diagnostics"]
            .as_array_mut()
            .ok_or_else(|| {
                runtime_store_corrupt(path, "Runtime diagnostics must be an array".into())
            })?
            .push(
                serde_json::to_value(diagnostic)
                    .map_err(|error| runtime_store_corrupt(path, error.to_string()))?,
            );
        document["next_diagnostic_id"] = serde_json::json!(next);
    }
    Ok(())
}
