//! Narrow Ticket owner adapter. Resource selectors never replace current
//! Subject/delegation authorization; values are projected explicitly and never
//! serialized from the general Ticket detail (which contains thread/admin data).
use super::*;
use crate::plugin_catalog::{PluginReference, PluginReferenceKind};
use crate::plugin_data as store;
use server_api::authorization::{OperationAuthorizationRequest, PermissionResource};

fn not_found() -> PluginHostError {
    policy::map_error(policy::failure(PluginHostErrorCode::NotFound))
}
fn invalid() -> PluginHostError {
    policy::map_error(policy::failure(PluginHostErrorCode::InvalidInput))
}

fn collection_granted(
    admitted: &policy::AdmittedPluginOperation,
    collection: &PluginDataCollection,
    write: bool,
) -> bool {
    admitted
        .pin
        .projection()
        .grants
        .iter()
        .any(|grant| match grant {
            crate::plugin_plan::PluginGrant::TicketDataRead {
                collection: c,
                schema_id,
                schema_version,
            } if !write => {
                c == &collection.collection
                    && schema_id == &collection.schema_id
                    && schema_version == &collection.schema_version
            }
            crate::plugin_plan::PluginGrant::TicketDataWrite {
                collection: c,
                schema_id,
                schema_version,
            } if write => {
                c == &collection.collection
                    && schema_id == &collection.schema_id
                    && schema_version == &collection.schema_version
            }
            _ => false,
        })
}

fn access_granted(admitted: &policy::AdmittedPluginOperation, access: PluginTicketAccess) -> bool {
    admitted
        .pin
        .projection()
        .grants
        .iter()
        .any(|grant| match (access, grant) {
            (
                PluginTicketAccess::Read,
                crate::plugin_plan::PluginGrant::TicketRead
                | crate::plugin_plan::PluginGrant::TicketDataRead { .. },
            ) => true,
            (
                PluginTicketAccess::ExtensionWrite,
                crate::plugin_plan::PluginGrant::TicketDataWrite { .. },
            ) => true,
            _ => false,
        })
}

pub(super) fn authorize_ticket(
    api: &WorkspaceApi,
    context: &ServerRequestContext,
    workspace: &str,
    reference: &str,
    access: PluginTicketAccess,
) -> crate::Result<String> {
    let hidden = || policy::failure(PluginHostErrorCode::NotFound);
    if reference.is_empty() || reference.len() > 512 || reference.chars().any(char::is_control) {
        return Err(hidden());
    }
    let worker = subject_authorization::source_from_context(context).map_err(|e| e.error)?;
    let ticket_id = api
        .store
        .resolve_resource_reference(workspace, WorkspaceResourceKind::Ticket, reference)?
        .ok_or_else(hidden)?;
    subject_authorization::authorize(
        api,
        &worker,
        &OperationAuthorizationRequest {
            operation: match access {
                PluginTicketAccess::Read => "ticket.read",
                PluginTicketAccess::ExtensionWrite => "ticket.extension.write",
            }
            .into(),
            resource: PermissionResource::Ticket {
                workspace_id: workspace.into(),
                ticket_id: ticket_id.clone(),
            },
        },
    )
    .map_err(|e| match e.error {
        Error::WorkspacePermissionDenied(_) => hidden(),
        other => other,
    })?;
    Ok(ticket_id)
}

fn read_ticket(
    tx: &rusqlite::Transaction<'_>,
    workspace: &str,
    ticket: &str,
) -> crate::Result<PluginTicketSnapshot> {
    let (title, status, body): (String, String, String) = tx.query_row(
        "SELECT t.title,t.status,e.body FROM typed_tickets t JOIN typed_ticket_events e ON e.workspace_id=t.workspace_id AND e.ticket_id=t.ticket_id AND e.event_index=-1 WHERE t.workspace_id=?1 AND t.ticket_id=?2 AND EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=t.workspace_id AND state='active')",
        rusqlite::params![workspace,ticket], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional()?.ok_or_else(|| policy::failure(PluginHostErrorCode::NotFound))?;
    let content_digest = ticket::sqlite_ticket_content_digest(tx, workspace, ticket)?
        .ok_or_else(|| policy::failure(PluginHostErrorCode::NotFound))?;
    let truncated = body.len() > PLUGIN_TICKET_BODY_LIMIT;
    let mut end = body.len().min(PLUGIN_TICKET_BODY_LIMIT);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let case_state = match status.as_str() {
        "open" => PluginTicketCaseState::Open,
        "closed" => PluginTicketCaseState::Closed,
        _ => return Err(policy::failure(PluginHostErrorCode::Unavailable)),
    };
    Ok(PluginTicketSnapshot {
        title,
        case_state,
        content_digest,
        body: body[..end].into(),
        truncated,
    })
}

fn reference(
    tx: &rusqlite::Transaction<'_>,
    workspace: &str,
    admitted: &policy::AdmittedPluginOperation,
) -> crate::Result<PluginReference> {
    let expected_last_operation = tx.query_row(
        "SELECT last_operation FROM plugin_installations WHERE workspace_id=?1 AND installation_id=?2 AND uninstalled=0",
        rusqlite::params![workspace,admitted.pin.installation_id()], |row| row.get(0),
    ).optional()?.ok_or_else(|| policy::failure(PluginHostErrorCode::Denied))?;
    Ok(PluginReference {
        kind: PluginReferenceKind::Data,
        owner: String::new(),
        installation_id: admitted.pin.installation_id().into(),
        expected_last_operation,
        package_digest: admitted.pin.package_digest().into(),
        artifact_digest: admitted.pin.artifact_digest().into(),
    })
}

fn schema<'a>(
    admitted: &'a policy::AdmittedPluginOperation,
    collection: &PluginDataCollection,
) -> crate::Result<&'a PluginDataSchemaMetadata> {
    admitted
        .data_collections
        .iter()
        .find(|declaration| {
            declaration.collection == collection.collection
                && declaration.schema_id == collection.schema_id
                && declaration.schema_version == collection.schema_version
        })
        .ok_or_else(|| policy::failure(PluginHostErrorCode::Denied))
}
fn validate_value(
    declaration: &PluginDataSchemaMetadata,
    mutation: &PluginDataMutation,
) -> crate::Result<()> {
    if let PluginDataMutation::Put { value_json, .. } = mutation {
        if value_json.len() > PLUGIN_DATA_VALUE_LIMIT {
            return Err(policy::failure(PluginHostErrorCode::QuotaExceeded));
        }
        let value: serde_json::Value = serde_json::from_str(value_json)
            .map_err(|_| policy::failure(PluginHostErrorCode::InvalidInput))?;
        manifest::plugin::schema::validate_data_value(&declaration.schema, &value)
            .map_err(|_| policy::failure(PluginHostErrorCode::InvalidInput))?;
    }
    Ok(())
}

enum Action {
    Resolve(PluginTicketAccess),
    Read,
    Get(PluginDataAddress),
    Mutate(PluginDataAddress, String, PluginDataMutation),
    List(PluginDataCollection, Option<String>, u32),
    Lookup(PluginDataAddress, String),
}
impl Action {
    fn access(&self) -> PluginTicketAccess {
        match self {
            Self::Resolve(access) => *access,
            Self::Mutate(..) | Self::Lookup(..) => PluginTicketAccess::ExtensionWrite,
            _ => PluginTicketAccess::Read,
        }
    }
    fn collection(&self) -> Option<&PluginDataCollection> {
        match self {
            Self::Get(a) | Self::Mutate(a, ..) | Self::Lookup(a, ..) => Some(&a.collection),
            Self::List(c, ..) => Some(c),
            _ => None,
        }
    }
}

pub(super) async fn execute(
    api: Arc<WorkspaceApi>,
    context: ServerRequestContext,
    workspace: String,
    request: PluginHostRequest,
) -> std::result::Result<PluginHostResponse, PluginHostError> {
    let (invocation_id, selector, action) = match request {
        PluginHostRequest::ResolveTicket {
            invocation_id,
            ticket_ref,
            access,
        } => (invocation_id, ticket_ref, Action::Resolve(access)),
        PluginHostRequest::ReadTicket {
            invocation_id,
            ticket_id,
        } => (invocation_id, ticket_id, Action::Read),
        PluginHostRequest::DataGet {
            invocation_id,
            ticket_id,
            address,
        } => (invocation_id, ticket_id, Action::Get(address)),
        PluginHostRequest::DataMutate {
            invocation_id,
            ticket_id,
            address,
            operation_key,
            mutation,
        } => (
            invocation_id,
            ticket_id,
            Action::Mutate(address, operation_key, mutation),
        ),
        PluginHostRequest::DataList {
            invocation_id,
            ticket_id,
            collection,
            cursor,
            limit,
        } => (
            invocation_id,
            ticket_id,
            Action::List(collection, cursor, limit),
        ),
        PluginHostRequest::DataLookupMutation {
            invocation_id,
            ticket_id,
            address,
            operation_key,
        } => (
            invocation_id,
            ticket_id,
            Action::Lookup(address, operation_key),
        ),
        _ => return Err(invalid()),
    };
    // Lookup after expiry must remain distinguishable from NotRecorded. It still
    // goes through the exact same live owner checks and current Ticket authority.
    let config = api.config_store.clone();
    let w = workspace.clone();
    let id = invocation_id.clone();
    let mut record = config
        .db_executor()
        .run(move || config.with_conn(|conn| receipt::lookup(conn, &w, &id)))
        .await
        .map_err(policy::map_error)?;
    record.service_lease = service_lease();
    let admitted = check(
        api.clone(),
        context.clone(),
        workspace.clone(),
        record.plan_id.clone(),
        record.installation_id.clone(),
        record.consumer.clone(),
        PluginHostOperation::Admit,
    )
    .await?;
    if record.execution != *admitted.plan.plan.execution() {
        return Err(denied());
    }
    if !access_granted(&admitted, action.access()) {
        return Err(not_found());
    }
    let config = api.config_store.clone();
    let executor = config.db_executor();
    executor.run(move || {
        // Resolve aliases and the live Subject/delegation ceiling before any
        // Ticket content or receipt is read. The DB transaction then fences
        // catalog revoke, cancellation, parent deletion and competing writes.
        let ticket_id = authorize_ticket(&api, &context, &workspace, &selector, action.access())?;
        config.with_immediate_transaction(|tx| {
            let record = receipt::verify_record(tx, &record)?;
            let admitted = match &record.consumer {
                PluginConsumer::ToolCall { tool_name, .. } => policy::authorize(tx, &workspace, &record.execution, &record.plan_id, &record.installation_id, &[tool_name.clone()], &PluginHostOperation::Admit)?,
                PluginConsumer::Consumer {export_name, ..} => policy::authorize_service(tx, &workspace, &record.execution, &record.plan_id, &record.installation_id, export_name, &PluginHostOperation::Admit)?,
            };
            if record.cancelled { return Err(policy::failure(PluginHostErrorCode::Cancelled)); }
            if !matches!(action, Action::Lookup(..)) && (record.expired || record.expires_at_ms <= now_ms()) { return Err(policy::failure(PluginHostErrorCode::ReceiptExpired)); }
            if !tx.query_row("SELECT EXISTS(SELECT 1 FROM typed_tickets t JOIN workspaces w USING(workspace_id) WHERE t.workspace_id=?1 AND t.ticket_id=?2 AND w.state='active')",rusqlite::params![workspace,ticket_id],|row|row.get::<_,bool>(0))? { return Err(policy::failure(PluginHostErrorCode::NotFound)); }
            if let Some(collection) = action.collection() {
                let write = matches!(action, Action::Mutate(..) | Action::Lookup(..));
                if !collection_granted(&admitted, collection, write) { return Err(policy::failure(PluginHostErrorCode::Denied)); }
                schema(&admitted, collection)?;
            }
            match action {
                Action::Resolve(_) => Ok(PluginHostResponse::TicketResolved { ticket_id }),
                Action::Read => {
                    if !admitted.pin.projection().grants.contains(&crate::plugin_plan::PluginGrant::TicketRead) { return Err(policy::failure(PluginHostErrorCode::Denied)); }
                    Ok(PluginHostResponse::TicketRead { snapshot: read_ticket(tx, &workspace, &ticket_id)? })
                }
                Action::Get(address) => Ok(PluginHostResponse::DataCell { cell: store::get(tx, &store::namespace(&record, &ticket_id, &address), &address.key)? }),
                Action::Mutate(address, operation_key, mutation) => {
                    validate_value(schema(&admitted, &address.collection)?, &mutation)?;
                    let receipt = store::mutate(tx, &record, &store::namespace(&record, &ticket_id, &address), &address.key, &operation_key, &mutation, &reference(tx, &workspace, &admitted)?, now_ms())?;
                    Ok(PluginHostResponse::DataMutated { receipt })
                }
                Action::List(collection, cursor, limit) => {
                    let namespace = store::Namespace { workspace_id: workspace, installation_id: record.installation_id, ticket_id, collection };
                    Ok(PluginHostResponse::DataListed { page: store::list(tx, &namespace, cursor.as_deref(), limit, now_ms())? })
                }
                Action::Lookup(address, operation_key) => Ok(PluginHostResponse::DataMutationLookup { outcome: store::lookup_mutation(tx, &record, &store::namespace(&record, &ticket_id, &address), &address.key, &operation_key, now_ms())? }),
            }
        })
    }).await.map_err(policy::map_error)
}
