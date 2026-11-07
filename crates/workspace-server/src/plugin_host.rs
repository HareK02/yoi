//! Server execution policy composes authenticated Host identity with retained plan
//! ceilings. Catalog management, artifact possession and ordinary configuration
//! selection never issue execution authority. Callers must run the existing live
//! operation/delegation authorizer before this same-database boundary.

use crate::{Error, Result, plugin_binding::PluginConnectionBinding, plugin_plan::*};
use rusqlite::{OptionalExtension, Transaction, params};
use server_api::*;
use sha2::{Digest, Sha256};

pub(crate) fn failure(code: PluginHostErrorCode) -> Error {
    PluginHostError {
        code,
        effect: PluginEffectObservation::NotDispatched,
        diagnostic_id: uuid::Uuid::now_v7().to_string(),
    }
    .into()
}

/// This value stays in the Server. It is never a wire connection capability.
#[derive(Clone)]
pub struct AdmittedPluginOperation {
    pub plan: StoredPluginPlan,
    pub pin: WorkerPluginPin,
    pub binding: Option<PluginConnectionBinding>,
    pub data_collections: Vec<PluginDataSchemaMetadata>,
    pub services: Vec<PluginServiceMetadata>,
}

/// `execution` is derived by the authenticated Host boundary from current Worker
/// registration, Subject policy and delegation lineage, never deserialized input.
/// Reads deliberately use the current catalog operation, not the launch-time one:
/// adding another artifact must not invalidate an existing immutable pin.
pub fn authorize(
    tx: &Transaction<'_>,
    workspace: &str,
    execution: &HostExecutionIdentity,
    plan_id: &str,
    installation: &str,
    tools: &[String],
    operation: &PluginHostOperation,
) -> Result<AdmittedPluginOperation> {
    authorize_selected(
        tx,
        workspace,
        execution,
        plan_id,
        installation,
        tools,
        None,
        operation,
    )
}
pub fn authorize_service(
    tx: &Transaction<'_>,
    workspace: &str,
    execution: &HostExecutionIdentity,
    plan_id: &str,
    installation: &str,
    service: &str,
    operation: &PluginHostOperation,
) -> Result<AdmittedPluginOperation> {
    authorize_selected(
        tx,
        workspace,
        execution,
        plan_id,
        installation,
        &[],
        Some(service),
        operation,
    )
}
fn authorize_selected(
    tx: &Transaction<'_>,
    workspace: &str,
    execution: &HostExecutionIdentity,
    plan_id: &str,
    installation: &str,
    tools: &[String],
    service: Option<&str>,
    operation: &PluginHostOperation,
) -> Result<AdmittedPluginOperation> {
    let plan = plugin_plan_load(tx, workspace, plan_id)?;
    if plan.plan.execution() != execution || plan.pin_state == PlanPinState::Released {
        return Err(failure(PluginHostErrorCode::Denied));
    }
    let pin = plan
        .plan
        .plugins()
        .iter()
        .find(|pin| pin.installation_id() == installation)
        .cloned()
        .ok_or_else(|| failure(PluginHostErrorCode::Denied))?;
    if service.is_none()
        && (tools.is_empty()
            || tools.len() > 256
            || tools.iter().any(|tool| {
                tool.is_empty() || tool.len() > 512 || tool.chars().any(char::is_control)
            })
            || tools
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != tools.len())
    {
        return Err(failure(PluginHostErrorCode::InvalidInput));
    }
    if let Some(service) = service {
        if service.is_empty()
            || service.len() > 512
            || service.chars().any(char::is_control)
            || !pin.projection().surfaces.contains(&PluginSurface::Service)
            || !pin.projection().services.iter().any(|s| s == service)
            || !pin.projection().grants.contains(&PluginGrant::TicketEvents)
        {
            return Err(failure(PluginHostErrorCode::Denied));
        }
    } else if !pin.projection().surfaces.contains(&PluginSurface::Tool)
        || tools
            .iter()
            .any(|tool| !pin.projection().tools.contains(tool))
    {
        return Err(failure(PluginHostErrorCode::Denied));
    }
    let expected_kind = match plan.pin_state {
        PlanPinState::Pending => "pending",
        PlanPinState::Active => "active",
        PlanPinState::Restorable => "restorable",
        PlanPinState::Released => unreachable!(),
    };
    let retained: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_references WHERE workspace_id=?1 AND kind=?2 AND owner=?3 AND installation_id=?4 AND artifact_digest=?5)",
        params![workspace, expected_kind, plan_id, installation, pin.artifact_digest()], |r| r.get(0),
    )?;
    if !retained {
        return Err(failure(PluginHostErrorCode::Denied));
    }
    let current: Option<String> = tx.query_row(
        "SELECT last_operation FROM plugin_installations WHERE workspace_id=?1 AND installation_id=?2 AND uninstalled=0",
        params![workspace, installation], |r| r.get(0),
    ).optional()?;
    let current = current.ok_or_else(|| failure(PluginHostErrorCode::Denied))?;
    let artifact = crate::plugin_catalog::plugin_artifact_admit(
        tx,
        workspace,
        installation,
        &current,
        pin.package_digest(),
        pin.artifact_digest(),
    )?;
    if !pin.matches_artifact(&artifact)? {
        return Err(failure(PluginHostErrorCode::Denied));
    }
    let slot = match operation {
        PluginHostOperation::Admit | PluginHostOperation::Publish => {
            for reference in &pin.projection().bindings {
                live_binding(tx, workspace, reference)?;
            }
            None
        }
        PluginHostOperation::Log => {
            if !pin.projection().grants.contains(&PluginGrant::Logging) {
                return Err(failure(PluginHostErrorCode::Denied));
            }
            None
        }
        PluginHostOperation::Open { slot } | PluginHostOperation::Send { slot } => Some(slot),
    };
    let binding = if let Some(slot) = slot {
        if !pin
            .projection()
            .grants
            .contains(&PluginGrant::Https { slot: slot.clone() })
        {
            return Err(failure(PluginHostErrorCode::Denied));
        }
        let reference = pin
            .projection()
            .bindings
            .iter()
            .find(|r| &r.slot == slot)
            .ok_or_else(|| failure(PluginHostErrorCode::Denied))?;
        Some(live_binding(tx, workspace, reference)?)
    } else {
        None
    };
    Ok(AdmittedPluginOperation {
        plan,
        pin,
        binding,
        data_collections: artifact.data_collections,
        services: artifact.services,
    })
}

pub(crate) fn live_binding(
    tx: &Transaction<'_>,
    workspace: &str,
    reference: &BindingPolicyReference,
) -> Result<PluginConnectionBinding> {
    let state = crate::config_source::load_state(tx, workspace)?
        .ok_or_else(|| failure(PluginHostErrorCode::Unavailable))?;
    let evaluation = config_source::SnapshotEnvironment::new(state.snapshot)
        .evaluate_contract(&state.contract)
        .map_err(|_| failure(PluginHostErrorCode::Unavailable))?;
    let settings = crate::plugin_settings::settings_from_evaluation(&evaluation)?;
    let binding = settings
        .bindings
        .get(&reference.binding_id)
        .ok_or_else(|| failure(PluginHostErrorCode::Denied))?
        .clone();
    binding
        .validate(&crate::plugin_binding::PluginConnectionPolicyCeiling::default())
        .map_err(|_| failure(PluginHostErrorCode::Denied))?;
    if binding.digest() != reference.policy_digest {
        return Err(failure(PluginHostErrorCode::Denied));
    }
    Ok(binding)
}

/// Only request identity is persisted, never request body, credential values or
/// transient deadlines. Query/header ordering remains part of protocol identity.
pub fn request_fingerprint(
    admitted: &AdmittedPluginOperation,
    slot: &str,
    request: &PluginHttpRequest,
) -> Result<String> {
    let reference = admitted
        .pin
        .projection()
        .bindings
        .iter()
        .find(|r| r.slot == slot)
        .ok_or_else(|| failure(PluginHostErrorCode::Denied))?;
    let body_digest: String = Sha256::digest(&request.body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let content = serde_json::to_vec(&(
        "yoi-plugin-effect-v1",
        &admitted.plan.plan_id,
        admitted.pin.installation_id(),
        &reference.binding_id,
        slot,
        &reference.policy_digest,
        &request.method,
        &request.path,
        &request.query,
        &request.headers,
        body_digest,
    ))
    .map_err(|_| failure(PluginHostErrorCode::InvalidInput))?;
    let hex: String = Sha256::digest(content)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(format!("sha256:{hex}"))
}

pub(crate) fn verify_receipt_schema(conn: &rusqlite::Connection) -> Result<()> {
    let expected = rusqlite::Connection::open_in_memory()?;
    expected.execute_batch(crate::plugin_receipt::SCHEMA_SQL)?;
    let mut statement = expected.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND tbl_name IN ('plugin_invocations','plugin_effect_receipts') ORDER BY name")?;
    for item in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (name, sql) = item?;
        let actual: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [&name],
                |row| row.get(0),
            )
            .optional()?;
        if actual.as_deref() != Some(&sql) {
            return Err(Error::Store("Plugin receipt schema has drifted".into()));
        }
    }
    Ok(())
}

pub(crate) fn map_error(error: Error) -> PluginHostError {
    match error {
        Error::PluginHost(error) => error,
        Error::PluginCatalog(_) | Error::WorkspacePermissionDenied(_) => {
            if let Error::PluginHost(e) = failure(PluginHostErrorCode::Denied) {
                e
            } else {
                unreachable!()
            }
        }
        _ => PluginHostError {
            code: PluginHostErrorCode::Unavailable,
            effect: PluginEffectObservation::NotDispatched,
            diagnostic_id: uuid::Uuid::now_v7().to_string(),
        },
    }
}
