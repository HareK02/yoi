//! Durable launch policy authority in the existing Workspace SQLite database.
//!
//! Store transactions own mutation and publication ordering. Catalog declarations
//! are projections only: admission and later effects must re-read the policy and
//! grant lineage, then check the current resolver and each Feature's resources.
//! Runtime dispatch is not part of this SQL transaction.

use feature_permissions::{PermissionDefinition, PermissionKey};
use rusqlite::{Connection, OptionalExtension, params};
use server_api::authorization::{
    CoreWorkerExecutionGrant, CoreWorkerGrantParent, CoreWorkerGrantSource,
    PermissionReferenceEncoding,
};
use server_api::launch_policy::*;

use crate::{Error, Result};

pub(crate) const SCHEMA: &str = include_str!("launch_policy.sql");
pub(crate) const CURRENT_SCHEMA: &str = include_str!("launch_policy_current.sql");
pub(crate) const MIGRATION_NAME: &str = "Workspace launch policies and core Worker grant issuance";

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidInput(message.into())
}
fn encode<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::Store(e.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T> {
    serde_json::from_str(raw).map_err(|e| Error::Store(e.to_string()))
}

pub(crate) fn require_owner(conn: &Connection, workspace: &str, account: &str) -> Result<()> {
    let allowed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=?1 AND owner_account_id=?2 AND state='active')",
        params![workspace, account], |row| row.get(0),
    )?;
    if !allowed {
        return Err(Error::WorkspacePermissionDenied(
            "Launch policy management requires the active Workspace owner".into(),
        ));
    }
    Ok(())
}

// Only current-policy readers project to V2. Receipt decoding and core grant
// readers must retain raw representation for historical replay/binding equality.
fn decode_current_policy(raw: &str) -> Result<WorkerLaunchPolicy> {
    let mut policy: WorkerLaunchPolicy = decode(raw)?;
    policy.child_permissions = policy.effective_child_permissions();
    policy.reference_encoding = PermissionReferenceEncoding::FeatureSetsV2;
    Ok(policy)
}

pub(crate) fn policy(
    conn: &Connection,
    workspace: &str,
    id: &str,
) -> Result<Option<WorkerLaunchPolicy>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT policy_json FROM worker_launch_policies WHERE workspace_id=?1 AND policy_id=?2",
            params![workspace, id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|s| decode_current_policy(&s)).transpose()
}

pub(crate) fn policies(conn: &Connection, workspace: &str) -> Result<Vec<WorkerLaunchPolicy>> {
    let mut statement = conn.prepare(
        "SELECT policy_json FROM worker_launch_policies WHERE workspace_id=?1 ORDER BY policy_id",
    )?;
    statement
        .query_map([workspace], |row| row.get::<_, String>(0))?
        .map(|raw| decode_current_policy(&raw?))
        .collect()
}

/// Owner authentication is checked even for historical replays. The entire typed
/// input and actor are compared before returning the original immutable receipt.
/// Caller must hold an IMMEDIATE transaction through this function and commit.
pub(crate) fn mutate(
    conn: &Connection,
    workspace: &str,
    account: &str,
    request: &WorkerLaunchPolicyMutation,
) -> Result<WorkerLaunchPolicyReceipt> {
    require_owner(conn, workspace, account)?;
    request.validate().map_err(invalid)?;
    let old: Option<(String, String, String)> = conn.query_row(
        "SELECT actor_account_id,request_json,receipt_json FROM worker_launch_policy_operations WHERE workspace_id=?1 AND operation_key=?2",
        params![workspace, request.operation_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional()?;
    if let Some((actor, raw, receipt)) = old {
        if actor != account || decode::<WorkerLaunchPolicyMutation>(&raw)? != *request {
            return Err(invalid(
                "Launch policy operation key reused with different full input or actor",
            ));
        }
        return decode(&receipt);
    }
    let previous = policy(conn, workspace, &request.policy_id)?;
    if previous.as_ref().map(WorkerLaunchPolicy::observation) != request.expected {
        return Err(invalid(
            "Launch policy CAS conflict; reread authorization key and operation key",
        ));
    }
    let mut next = match (&request.action, previous) {
        (
            WorkerLaunchPolicyAction::Create {
                name,
                allowed_profiles,
                child_permissions,
            },
            None,
        ) => WorkerLaunchPolicy {
            policy_id: request.policy_id.clone(),
            name: name.clone(),
            authorization_key: format!("operation:{}", request.operation_key),
            state: WorkerLaunchPolicyState::Active,
            allowed_profiles: allowed_profiles.clone(),
            child_permissions: child_permissions.clone(),
            reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
            last_operation_key: request.operation_key.clone(),
        },
        (WorkerLaunchPolicyAction::Create { .. }, Some(_)) => {
            return Err(invalid(
                "Launch policy ID is permanently reserved, including tombstones",
            ));
        }
        (_, None) => return Err(invalid("Launch policy not found")),
        (action, Some(mut p)) => {
            if p.state == WorkerLaunchPolicyState::Deleted {
                return Err(invalid("Deleted launch policy ID cannot be reused"));
            }
            match action {
                WorkerLaunchPolicyAction::Rename { name } => p.name = name.clone(),
                WorkerLaunchPolicyAction::ReplaceAuthorization {
                    allowed_profiles,
                    child_permissions,
                } => {
                    p.authorization_key = format!("operation:{}", request.operation_key);
                    p.state = WorkerLaunchPolicyState::Active;
                    p.allowed_profiles = allowed_profiles.clone();
                    p.child_permissions = child_permissions.clone();
                }
                WorkerLaunchPolicyAction::Revoke => {
                    p.authorization_key = format!("operation:{}", request.operation_key);
                    p.state = WorkerLaunchPolicyState::Revoked;
                }
                WorkerLaunchPolicyAction::Delete => {
                    p.authorization_key = format!("operation:{}", request.operation_key);
                    p.state = WorkerLaunchPolicyState::Deleted;
                }
                WorkerLaunchPolicyAction::Create { .. } => unreachable!(),
            }
            p
        }
    };
    next.last_operation_key = request.operation_key.clone();
    let state = match next.state {
        WorkerLaunchPolicyState::Active => "active",
        WorkerLaunchPolicyState::Revoked => "revoked",
        WorkerLaunchPolicyState::Deleted => "deleted",
    };
    let raw = encode(&next)?;
    conn.execute("INSERT INTO worker_launch_policies(workspace_id,policy_id,authorization_key,state,policy_json,last_operation_key) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(workspace_id,policy_id) DO UPDATE SET authorization_key=excluded.authorization_key,state=excluded.state,policy_json=excluded.policy_json,last_operation_key=excluded.last_operation_key",
        params![workspace,next.policy_id,next.authorization_key,state,raw,next.last_operation_key])?;
    conn.execute("INSERT INTO worker_launch_policy_history(workspace_id,policy_id,operation_key,authorization_key,policy_json) VALUES(?1,?2,?3,?4,?5)",params![workspace,next.policy_id,request.operation_key,next.authorization_key,raw])?;
    let receipt = WorkerLaunchPolicyReceipt {
        policy: next,
        operation_key: request.operation_key.clone(),
    };
    conn.execute("INSERT INTO worker_launch_policy_operations(workspace_id,operation_key,actor_account_id,request_json,receipt_json) VALUES(?1,?2,?3,?4,?5)",params![workspace,request.operation_key,account,encode(request)?,encode(&receipt)?])?;
    Ok(receipt)
}

pub(crate) fn require_current_policy(
    conn: &Connection,
    workspace: &str,
    id: &str,
    authorization_key: &str,
    profile: &str,
) -> Result<WorkerLaunchPolicy> {
    validate_canonical_profile(profile).map_err(invalid)?;
    let p = policy(conn, workspace, id)?
        .ok_or_else(|| invalid("Launch policy not found in this Workspace"))?;
    if p.state != WorkerLaunchPolicyState::Active || p.authorization_key != authorization_key {
        return Err(invalid(
            "Launch policy is revoked, deleted or has a different authorization key",
        ));
    }
    if !p.allowed_profiles.iter().any(|s| s == profile) {
        return Err(invalid(
            "Profile is not explicitly allowed by launch policy",
        ));
    }
    Ok(p)
}

/// No set contribution is inferred. The Feature owner combines these exact keys
/// with its ordinary declarations using its existing namespace handle.
pub fn usage_declarations(policies: &[WorkerLaunchPolicy]) -> Result<Vec<PermissionDefinition>> {
    policies
        .iter()
        .filter(|p| p.state == WorkerLaunchPolicyState::Active)
        .map(|p| {
            Ok(PermissionDefinition {
                key: p.usage_key().map_err(invalid)?,
                description: format!("Launch using policy {}", p.policy_id),
                children: vec![],
            })
        })
        .collect()
}

pub(crate) fn core_grant(
    conn: &Connection,
    workspace: &str,
    runtime: &str,
    worker: &str,
) -> Result<Option<CoreWorkerExecutionGrant>> {
    let raw: Option<String> = conn.query_row("SELECT grant_json FROM core_worker_execution_grants WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",params![workspace,runtime,worker],|r|r.get(0)).optional()?;
    raw.map(|s| decode(&s)).transpose()
}

/// Immutable idempotent binding. Host must already have authenticated the caller,
/// resolved policy usage + worker.create, validated the assignment with the current
/// resolver, and frozen the Profile bundle in the creation reservation. The caller
/// holds the same transaction as the reserved identity binding.
pub(crate) fn bind_core_grant(
    conn: &Connection,
    grant: &CoreWorkerExecutionGrant,
) -> Result<CoreWorkerExecutionGrant> {
    grant.validate().map_err(invalid)?;
    let mut statement = conn.prepare("SELECT grant_json FROM core_worker_execution_grants WHERE workspace_id=?1 AND (grant_id=?2 OR creation_operation_id=?3 OR (runtime_id=?4 AND worker_id=?5))")?;
    let existing = statement
        .query_map(
            params![
                grant.workspace_id,
                grant.grant_id,
                grant.creation_operation_id,
                grant.runtime_id,
                grant.worker_id
            ],
            |r| r.get::<_, String>(0),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if !existing.is_empty() {
        if existing.len() == 1 && decode::<CoreWorkerExecutionGrant>(&existing[0])? == *grant {
            // Historical binding is not current authorization. Dispatch/restore
            // must independently validate policy lineage and current references.
            return Ok(grant.clone());
        }
        return Err(invalid(
            "Core Worker grant identity/operation already bound to different full input",
        ));
    }
    let (policy_id, policy_authorization_key) = match &grant.source {
        CoreWorkerGrantSource::LaunchPolicy {
            policy_id,
            policy_authorization_key,
            ..
        } => {
            let p = require_current_policy(
                conn,
                &grant.workspace_id,
                policy_id,
                policy_authorization_key,
                &grant.profile,
            )?;
            if p.effective_child_permissions() != grant.effective_assignment() {
                return Err(invalid("Grant assignment differs from launch policy"));
            }
            (
                Some(policy_id.as_str()),
                Some(policy_authorization_key.as_str()),
            )
        }
        CoreWorkerGrantSource::Account { account_id } => {
            require_owner(conn, &grant.workspace_id, account_id)?;
            (None, None)
        }
        CoreWorkerGrantSource::Delegation { .. } => (None, None),
    };
    // The row is inserted only after lineage validation. Its own record need not
    // exist to validate a newly reserved child's parent chain.
    validate_policy_lineage(conn, grant)?;
    conn.execute("INSERT INTO core_worker_execution_grants(workspace_id,grant_id,creation_operation_id,runtime_id,worker_id,grant_json,policy_id,policy_authorization_key) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![grant.workspace_id,grant.grant_id,grant.creation_operation_id,grant.runtime_id,grant.worker_id,encode(grant)?,policy_id,policy_authorization_key])?;
    Ok(grant.clone())
}

/// Checks current policy + core ancestry, returning external Subject roots which
/// Host must validate through existing Subject policy/body/legacy containment
/// authority. This is not a complete operation authorization decision: current
/// reference resolution, finite ceilings and resource constraints remain separate.
pub(crate) fn validate_policy_lineage(
    conn: &Connection,
    grant: &CoreWorkerExecutionGrant,
) -> Result<Vec<CoreWorkerGrantParent>> {
    let mut cursor = grant.clone();
    let mut seen = std::collections::BTreeSet::new();
    loop {
        cursor.validate().map_err(invalid)?;
        if !seen.insert(cursor.grant_id.clone()) || seen.len() > 256 {
            return Err(invalid("Core Worker grant lineage cycle or depth limit"));
        }
        let revoked: Option<bool> = conn.query_row("SELECT revoked FROM core_worker_execution_grants WHERE workspace_id=?1 AND grant_id=?2",params![cursor.workspace_id,cursor.grant_id],|r|r.get(0)).optional()?;
        if revoked == Some(true) {
            return Err(invalid("Core Worker grant revoked"));
        }
        let parent = match &cursor.source {
            CoreWorkerGrantSource::Account { .. } => {
                return Ok(vec![]);
            }
            CoreWorkerGrantSource::Delegation { parent } => parent,
            CoreWorkerGrantSource::LaunchPolicy {
                policy_id,
                policy_authorization_key,
                parent,
            } => {
                let p = require_current_policy(
                    conn,
                    &cursor.workspace_id,
                    policy_id,
                    policy_authorization_key,
                    &cursor.profile,
                )?;
                if p.effective_child_permissions() != cursor.effective_assignment() {
                    return Err(invalid("Core Worker grant policy assignment mismatch"));
                }
                parent
            }
        };
        match parent {
            CoreWorkerGrantParent::WorkerGrant {
                grant_id,
                runtime_id,
                worker_id,
            } => {
                let next = core_grant(conn, &cursor.workspace_id, runtime_id, worker_id)?
                    .ok_or_else(|| invalid("Parent core Worker grant missing"))?;
                if &next.grant_id != grant_id {
                    return Err(invalid("Parent core Worker grant identity mismatch"));
                }
                cursor = next;
            }
            _ => return Ok(vec![parent.clone()]),
        }
    }
}

pub(crate) fn revoke_core_grant(
    conn: &Connection,
    workspace: &str,
    account: &str,
    grant_id: &str,
    operation_key: &str,
) -> Result<()> {
    require_owner(conn, workspace, account)?;
    if operation_key.trim().is_empty() || operation_key.len() > 256 {
        return Err(invalid("Invalid core grant revocation operation key"));
    }
    let prior: Option<(String,String)> = conn.query_row("SELECT actor_account_id,grant_id FROM core_worker_grant_revocations WHERE workspace_id=?1 AND operation_key=?2",params![workspace,operation_key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((actor, target)) = prior {
        if actor != account || target != grant_id {
            return Err(invalid(
                "Core grant revocation operation reused with different input",
            ));
        }
        return Ok(());
    }
    if conn.execute(
        "UPDATE core_worker_execution_grants SET revoked=1 WHERE workspace_id=?1 AND grant_id=?2",
        params![workspace, grant_id],
    )? != 1
    {
        return Err(invalid("Core Worker grant not found"));
    }
    conn.execute("INSERT INTO core_worker_grant_revocations(workspace_id,operation_key,actor_account_id,grant_id) VALUES(?1,?2,?3,?4)",params![workspace,operation_key,account,grant_id])?;
    Ok(())
}

pub(crate) fn bounded_current_keys(
    grant: &CoreWorkerExecutionGrant,
    current: impl IntoIterator<Item = PermissionKey>,
) -> Vec<PermissionKey> {
    current
        .into_iter()
        .filter(|key| grant.bounds_operation(key))
        .collect()
}

#[cfg(test)]
mod tests;
