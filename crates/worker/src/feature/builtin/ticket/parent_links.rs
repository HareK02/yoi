//! Decomposition links reuse relation authority, not case/assignment or inherited grants.
//! Tool and WIP inputs converge here; the Backend owns atomic CAS, cycle checks and replay.
use super::*;

#[cfg(test)]
mod tests;

pub(super) const TOOL_NAMES: &[&str] = &["TicketParentSet", "TicketChildrenQuery"];

#[derive(Clone, Copy)]
pub(super) enum ParentToolKind {
    Set,
    Children,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetInput {
    /// Child Ticket reference. WIP binds this from the addressed item route.
    ticket: String,
    /// Exact observation from ShowTicket.parent_link; both nullable fields must be present.
    expected: ticket::TicketParentObservation,
    /// Desired parent reference, or explicit null to unlink. Never omit.
    #[serde(deserialize_with = "deserialize_required_parent")]
    #[schemars(required, schema_with = "nullable_string_schema")]
    parent: Option<String>,
    /// Stable exact-replay key; retain the full original selectors and input on retry.
    operation_key: String,
}

fn nullable_string_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    Option::<String>::json_schema(generator)
}

fn deserialize_required_parent<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ChildrenInput {
    /// Parent Ticket reference. WIP binds this from the addressed item route.
    ticket: String,
    /// Opaque cursor from this parent's previous page; keep filters and scope unchanged.
    cursor: Option<String>,
    /// Bounded direct children, 1..=100; defaults to 50. Not a recursive tree expansion.
    #[schemars(range(min = 1, max = 100))]
    limit: Option<usize>,
}

impl ParentToolKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Set => TOOL_NAMES[0],
            Self::Children => TOOL_NAMES[1],
        }
    }
    fn schema(self) -> Value {
        match self {
            Self::Set => json!(schemars::schema_for!(SetInput)),
            Self::Children => json!(schemars::schema_for!(ChildrenInput)),
        }
    }
}

pub(super) fn description(name: &str) -> &'static str {
    match name {
        "TicketParentSet" => {
            "Atomically set, replace or unlink this Ticket's parent. Reread ShowTicket.parent_link and supply its complete expected observation, including explicit nullable parent_ticket_id and last_operation_key; the retained key fences ABA even after unlink. parent must be a Ticket reference or explicit null. Exact replay retains the same operation_key, selectors and full input and returns a historical receipt, not a fresh observation. On stale/CAS conflict reread and reconcile; on unknown outcome reconcile saved authority and retry only the exact original request, never automatically mint a new key. Requires relation.read on child and desired parent metadata before mutation, independently of mutation grants. Setting requires relation.record on child and desired parent. Unlinking, or replacing an existing old parent with a different resolved canonical ID, additionally requires relation.remove on child and expected old parent when present. First attachment and the same canonical parent require no remove grant. Links do not grant resources, inherit authority, change case/assignment, start Workers or imply parent fulfillment."
        }
        "TicketChildrenQuery" => {
            "Read a bounded stable page of this Ticket's direct children, with case state and closure reason (closed is not necessarily success). Defaults to 50, maximum 100; has_more/next_cursor explicitly mark truncation. Continue with the opaque cursor and unchanged parent/scope/limit. Requires relation.read on the addressed parent and actual returned child Tickets; collection permission grants no linked-Ticket authority. No recursive expansion, inherited grants, case/assignment changes or execution rights."
        }
        _ => unreachable!("unknown parent link tool"),
    }
}

fn required(name: &str, value: &str) -> TicketResult<()> {
    if value.trim().is_empty() {
        return Err(TicketError::Conflict(format!("{name} must not be empty")));
    }
    Ok(())
}

/// Validate without normalizing selectors or keys: exact replay fingerprints the original input.
pub(super) fn validate_set(request: &ticket::TicketParentSetRequest) -> TicketResult<()> {
    required("operation_key", &request.operation_key)?;
    for (name, value) in [
        (
            "expected.parent_ticket_id",
            &request.expected.parent_ticket_id,
        ),
        (
            "expected.last_operation_key",
            &request.expected.last_operation_key,
        ),
        ("parent", &request.parent),
    ] {
        if let Some(value) = value {
            required(name, value)?;
        }
    }
    Ok(())
}

/// Resolve immutable identity using only this Ticket's metadata-read authority. Detail
/// would project parent metadata and require unrelated ancestor reads; the plain record
/// would additionally require body-read authority. Neither is needed for ID comparison.
/// The original CAS/replay input stays untouched.
fn resolve_id(
    client: Arc<dyn WorkspaceClient>,
    base: &str,
    reference: &TicketIdOrSlug,
) -> TicketResult<String> {
    authorize_ticket_item(client.as_ref(), "ticket.relation.read", reference)?;
    let summary: ticket::TicketLinkSummary = WorkspaceHttpTicketBackend::request_unprojected(
        client,
        WorkspaceRequestMethod::Get,
        format!(
            "{base}/{}/link-summary",
            WorkspaceHttpTicketBackend::ticket_path(reference),
        ),
        None,
    )?;
    if summary.id.is_empty() {
        return Err(TicketError::Conflict(
            "resolved Ticket identity is unavailable".into(),
        ));
    }
    Ok(summary.id)
}

pub(super) fn authorize_set(
    client: Arc<dyn WorkspaceClient>,
    base: &str,
    child: &TicketIdOrSlug,
    request: &ticket::TicketParentSetRequest,
) -> TicketResult<()> {
    let mut references = vec![("ticket.relation.read", child.clone())];
    authorize_ticket_item(client.as_ref(), "ticket.relation.read", child)?;
    let desired = if let Some(parent) = &request.parent {
        let parent = TicketIdOrSlug::from(parent.as_str());
        for reference in [child, &parent] {
            authorize_ticket_item(client.as_ref(), "ticket.relation.record", reference)?;
            references.push(("ticket.relation.record", reference.clone()));
        }
        // The receipt exposes desired-parent metadata. Mutation authority is not read
        // authority, so reject before PUT instead of denying after a successful write.
        authorize_ticket_item(client.as_ref(), "ticket.relation.read", &parent)?;
        references.push(("ticket.relation.read", parent.clone()));
        if request.expected.parent_ticket_id.is_some() {
            Some(resolve_id(client.clone(), base, &parent)?)
        } else {
            // First attachment has no old edge to remove. No identity/body lookup is
            // needed to decide its grants; Host authorization resolves the references.
            None
        }
    } else {
        None
    };
    let remove = request.parent.is_none()
        || (request.expected.parent_ticket_id.is_some()
            && request.expected.parent_ticket_id != desired);
    if remove {
        authorize_ticket_item(client.as_ref(), "ticket.relation.remove", child)?;
        references.push(("ticket.relation.remove", child.clone()));
        if let Some(old) = &request.expected.parent_ticket_id {
            let old = TicketIdOrSlug::from(old.as_str());
            authorize_ticket_item(client.as_ref(), "ticket.relation.remove", &old)?;
            references.push(("ticket.relation.remove", old));
        }
    }
    // Recheck the explicit resources immediately before the write. The resolved desired
    // ID is additionally guarded when comparison required a metadata lookup. Do not
    // rewrite any wire selector, observation or replay key with resolved identity.
    for (operation, reference) in references {
        authorize_ticket_item(client.as_ref(), operation, &reference)?;
    }
    if let Some(desired) = desired {
        let desired = TicketIdOrSlug::Id(desired);
        authorize_ticket_item(client.as_ref(), "ticket.relation.record", &desired)?;
        authorize_ticket_item(client.as_ref(), "ticket.relation.read", &desired)?;
    }
    Ok(())
}

pub(super) fn children_endpoint(
    base: &str,
    id: &TicketIdOrSlug,
    request: &ticket::TicketChildrenRequest,
) -> TicketResult<String> {
    let limit = request.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(TicketError::Conflict("limit must be in 1..=100".into()));
    }
    let mut endpoint = format!(
        "{base}/{}/children?limit={limit}",
        WorkspaceHttpTicketBackend::ticket_path(id),
    );
    if let Some(cursor) = &request.cursor {
        required("cursor", cursor)?;
        endpoint.push_str("&cursor=");
        endpoint.push_str(&WorkspaceHttpTicketBackend::ticket_path(
            &TicketIdOrSlug::Id(cursor.clone()),
        ));
    }
    Ok(endpoint)
}

pub(super) fn authorize_link(
    client: &dyn WorkspaceClient,
    link: &ticket::TicketParentLink,
) -> TicketResult<()> {
    authorize_ticket_item(
        client,
        "ticket.relation.read",
        &TicketIdOrSlug::from(link.ticket_id.as_str()),
    )?;
    if let Some(parent) = &link.parent {
        authorize_ticket_item(
            client,
            "ticket.relation.read",
            &TicketIdOrSlug::from(parent.id.as_str()),
        )?;
    }
    Ok(())
}

pub(super) fn project_detail(
    client: &dyn WorkspaceClient,
    value: Value,
) -> Result<Value, ToolError> {
    let parent_link: ticket::TicketParentLink =
        serde_json::from_value(value.get("parent_link").cloned().ok_or_else(|| {
            ToolError::ExecutionFailed("Ticket detail is missing parent_link".into())
        })?)
        .map_err(|error| {
            ToolError::ExecutionFailed(format!("decode Ticket parent_link: {error}"))
        })?;
    authorize_link(client, &parent_link)
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
    let detail = project_ticket_detail(value).map_err(ToolError::ExecutionFailed)?;
    serde_json::to_value(detail).map_err(|error| ToolError::Internal(error.to_string()))
}

#[derive(Clone)]
struct ParentTool {
    client: Arc<dyn WorkspaceClient>,
    kind: ParentToolKind,
}

#[async_trait]
impl Tool for ParentTool {
    async fn execute(&self, input: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        let value = serde_json::from_str(input)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        let result = execute_value(self.client.clone(), self.kind.name(), value)
            .expect("registered parent tool")?;
        Ok(ToolOutput {
            summary: self.kind.name().into(),
            content: Some(result.to_string()),
            attachments: Vec::new(),
        })
    }
}

pub(super) fn execute_value(
    client: Arc<dyn WorkspaceClient>,
    name: &str,
    input: Value,
) -> Option<Result<Value, ToolError>> {
    if !TOOL_NAMES.contains(&name) {
        return None;
    }
    Some((|| {
        let backend = WorkspaceHttpTicketBackend::new(client);
        let parse = |error: serde_json::Error| ToolError::InvalidArgument(error.to_string());
        let failure = |error: TicketError| ToolError::ExecutionFailed(error.to_string());
        let invalid = |error: TicketError| ToolError::InvalidArgument(error.to_string());
        match name {
            "TicketParentSet" => {
                let input: SetInput = serde_json::from_value(input).map_err(parse)?;
                required("ticket", &input.ticket).map_err(invalid)?;
                let request = ticket::TicketParentSetRequest {
                    expected: input.expected,
                    parent: input.parent,
                    operation_key: input.operation_key,
                };
                validate_set(&request).map_err(invalid)?;
                let result = backend
                    .set_ticket_parent(TicketIdOrSlug::from(input.ticket.as_str()), request)
                    .map_err(failure)?;
                Ok(json!(result))
            }
            "TicketChildrenQuery" => {
                let input: ChildrenInput = serde_json::from_value(input).map_err(parse)?;
                required("ticket", &input.ticket).map_err(invalid)?;
                let request = ticket::TicketChildrenRequest {
                    cursor: input.cursor,
                    limit: input.limit,
                };
                let id = TicketIdOrSlug::from(input.ticket.as_str());
                children_endpoint("", &id, &request).map_err(invalid)?;
                let result = backend
                    .query_ticket_children(id, request)
                    .map_err(failure)?;
                Ok(json!(result))
            }
            _ => unreachable!(),
        }
    })())
}

pub(super) fn definition(client: Arc<dyn WorkspaceClient>, kind: ParentToolKind) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(kind.name())
                .description(description(kind.name()))
                .input_schema(kind.schema()),
            Arc::new(ParentTool {
                client: client.clone(),
                kind,
            }) as Arc<dyn Tool>,
        )
    })
}
