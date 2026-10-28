//! Classification tools share the Ticket feature's scoped Backend transport.
#[cfg(test)]
mod tests;
use super::*;

pub(super) const TAG_TOOL_NAMES: &[&str] = &[
    "WorkspaceTagList",
    "WorkspaceTagCreate",
    "WorkspaceTagRename",
    "WorkspaceTagDelete",
    "TicketTagsSet",
];

#[derive(Clone, Copy)]
pub(super) enum TagToolKind {
    List,
    Create,
    Rename,
    Delete,
    Set,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListInput {
    /// Continue strictly after this stable tag ID. Continue until an empty page.
    after: Option<String>,
    /// Maximum results, 0..=200; defaults to 200.
    #[schemars(range(max = 200))]
    limit: Option<usize>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateInput {
    /// Display name: nonempty after Unicode trimming, no controls, at most 128 UTF-8 bytes.
    name: String,
    /// Retry an uncertain request with this same key and exact input.
    operation_key: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameInput {
    tag_id: String,
    /// Last operation key observed on this exact identity; rename-back is still a change.
    expected_last_operation_key: String,
    name: String,
    operation_key: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteInput {
    tag_id: String,
    expected_last_operation_key: String,
    operation_key: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetInput {
    /// Ticket reference, preferably T-*; WIP binds it from the item route.
    ticket: String,
    /// Entire active ID set observed in ShowTicket/QueryTicket; never silently refreshed.
    #[schemars(length(max = 100))]
    expected_tag_ids: Vec<String>,
    /// Desired entire ID set. Attach/remove multiple tags atomically; [] removes all.
    #[schemars(length(max = 100))]
    tag_ids: Vec<String>,
    operation_key: String,
}

impl TagToolKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::List => "WorkspaceTagList",
            Self::Create => "WorkspaceTagCreate",
            Self::Rename => "WorkspaceTagRename",
            Self::Delete => "WorkspaceTagDelete",
            Self::Set => "TicketTagsSet",
        }
    }
    fn schema(self) -> Value {
        match self {
            Self::List => json!(schemars::schema_for!(ListInput)),
            Self::Create => json!(schemars::schema_for!(CreateInput)),
            Self::Rename => json!(schemars::schema_for!(RenameInput)),
            Self::Delete => json!(schemars::schema_for!(DeleteInput)),
            Self::Set => json!(schemars::schema_for!(SetInput)),
        }
    }
}

pub(super) fn description(name: &str) -> &'static str {
    match name {
        "WorkspaceTagList" => {
            "List active Workspace classification tags in stable ID order, bounded to 200. Continue after the last ID until exhausted; a full page does not promise another page."
        }
        "WorkspaceTagCreate" => {
            "Create a Workspace classification tag with a stable identity and replay key. Names are ordinary classification, never Ticket states, routing, or authority grants."
        }
        "WorkspaceTagRename" => {
            "Rename a Workspace classification tag by stable ID and observed last_operation_key. Links retain identity; history is not rewritten. Retry uncertain requests with the same key/input, then reread before further edits."
        }
        "WorkspaceTagDelete" => {
            "Tombstone a Workspace classification tag using its observed last_operation_key and atomically remove current Ticket membership. Prior history remains; recreating the name creates a new identity."
        }
        "TicketTagsSet" => {
            "Atomically compare and replace a Ticket's entire active classification tag ID set. Compute desired tag_ids from observed expected_tag_ids to attach/remove multiple tags without overwriting unrelated changes. At most 100 unique IDs. Classification does not edit state, content digest, assignment, or grants. A replay is the original receipt, not a fresh observation; reread before further edits."
        }
        _ => unreachable!("unknown classification tool"),
    }
}

fn required(name: &str, value: &str) -> Result<(), ToolError> {
    if value.trim().is_empty() {
        return Err(ToolError::InvalidArgument(format!(
            "{name} must not be empty"
        )));
    }
    Ok(())
}
fn validate_name(name: &str) -> Result<(), ToolError> {
    if name.trim().is_empty() || name.trim().len() > 128 || name.chars().any(char::is_control) {
        return Err(ToolError::InvalidArgument("name must be nonempty, contain no control characters, and be at most 128 UTF-8 bytes after trimming".into()));
    }
    Ok(())
}

pub(super) fn validate_ids(name: &str, ids: &[String]) -> Result<(), ToolError> {
    if ids.len() > 100 {
        return Err(ToolError::InvalidArgument(format!(
            "{name} must contain at most 100 IDs"
        )));
    }
    let mut unique = BTreeSet::new();
    for id in ids {
        required(name, id)?;
        if !unique.insert(id) {
            return Err(ToolError::InvalidArgument(format!(
                "{name} must contain unique IDs"
            )));
        }
    }
    Ok(())
}

#[derive(Clone)]
struct TagTool {
    client: Arc<dyn WorkspaceClient>,
    kind: TagToolKind,
}

// Internal command DTOs belong to the shared execution boundary, not a native schema.
enum Command {
    List(ListInput),
    Create(CreateInput),
    Rename(RenameInput),
    Delete(DeleteInput),
    Set(SetInput),
}

impl Command {
    fn kind(&self) -> TagToolKind {
        match self {
            Self::List(..) => TagToolKind::List,
            Self::Create(..) => TagToolKind::Create,
            Self::Rename(..) => TagToolKind::Rename,
            Self::Delete(..) => TagToolKind::Delete,
            Self::Set(..) => TagToolKind::Set,
        }
    }
}

#[async_trait]
impl Tool for TagTool {
    async fn execute(
        &self,
        input: &str,
        ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let command = match self.kind {
            TagToolKind::List => Command::List(
                serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?,
            ),
            TagToolKind::Create => Command::Create(
                serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?,
            ),
            TagToolKind::Rename => Command::Rename(
                serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?,
            ),
            TagToolKind::Delete => Command::Delete(
                serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?,
            ),
            TagToolKind::Set => Command::Set(
                serde_json::from_str(input)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?,
            ),
        };
        execute_domain(self.client.clone(), command, ctx).await
    }
}

/// Convert an adapter-owned internal command; never invokes Tool execution or its schema.
pub(super) async fn execute_native(
    client: Arc<dyn WorkspaceClient>,
    tool_name: &str,
    command: Value,
    ctx: ToolExecutionContext,
) -> Option<Result<ToolOutput, ToolError>> {
    let parse = |error: serde_json::Error| ToolError::InvalidArgument(error.to_string());
    let command = match tool_name {
        "WorkspaceTagList" => serde_json::from_value(command).map(Command::List),
        "WorkspaceTagCreate" => serde_json::from_value(command).map(Command::Create),
        "WorkspaceTagRename" => serde_json::from_value(command).map(Command::Rename),
        "WorkspaceTagDelete" => serde_json::from_value(command).map(Command::Delete),
        "TicketTagsSet" => serde_json::from_value(command).map(Command::Set),
        _ => return None,
    };
    Some(match command {
        Ok(command) => execute_domain(client, command, ctx).await,
        Err(error) => Err(parse(error)),
    })
}

async fn execute_domain(
    client: Arc<dyn WorkspaceClient>,
    command: Command,
    _ctx: ToolExecutionContext,
) -> Result<ToolOutput, ToolError> {
    let kind = command.kind();
    client
        .workspace_id()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            ToolError::InvalidArgument(
                "Classification tools require Backend Workspace identity; no local fallback".into(),
            )
        })?;
    let backend = WorkspaceHttpTicketBackend::new(client.clone());
    let content = match command {
        Command::List(input) => {
            if input.limit.is_some_and(|limit| limit > 200) {
                return Err(ToolError::InvalidArgument(
                    "limit must be at most 200".into(),
                ));
            }
            if let Some(after) = &input.after {
                required("after", after)?;
            }
            let limit = input.limit.unwrap_or(200);
            let mut tags = backend
                .list_workspace_tags(ticket::TagListRequest {
                    after: input.after,
                    limit: Some(limit),
                })
                .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
            tags.truncate(limit);
            json!(tags)
        }
        Command::Set(input) => {
            required("ticket", &input.ticket)?;
            required("operation_key", &input.operation_key)?;
            validate_ids("expected_tag_ids", &input.expected_tag_ids)?;
            validate_ids("tag_ids", &input.tag_ids)?;
            let tags = backend
                .set_ticket_tags(
                    TicketIdOrSlug::from(input.ticket.as_str()),
                    ticket::TicketTagSetRequest {
                        expected_tag_ids: input.expected_tag_ids,
                        tag_ids: input.tag_ids,
                        operation_key: input.operation_key,
                    },
                )
                .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
            if tags.len() > 100 {
                return Err(ToolError::ExecutionFailed(
                    "Backend returned more than 100 Ticket tags".into(),
                ));
            }
            json!(tags)
        }
        mutation @ (Command::Create(_) | Command::Rename(_) | Command::Delete(_)) => {
            let request = match mutation {
                Command::Create(input) => {
                    validate_name(&input.name)?;
                    ticket::WorkspaceTagMutation {
                        operation_key: input.operation_key,
                        action: ticket::WorkspaceTagAction::Create { name: input.name },
                    }
                }
                Command::Rename(input) => {
                    required("tag_id", &input.tag_id)?;
                    required(
                        "expected_last_operation_key",
                        &input.expected_last_operation_key,
                    )?;
                    validate_name(&input.name)?;
                    ticket::WorkspaceTagMutation {
                        operation_key: input.operation_key,
                        action: ticket::WorkspaceTagAction::Rename {
                            tag_id: input.tag_id,
                            expected_last_operation_key: input.expected_last_operation_key,
                            name: input.name,
                        },
                    }
                }
                Command::Delete(input) => {
                    required("tag_id", &input.tag_id)?;
                    required(
                        "expected_last_operation_key",
                        &input.expected_last_operation_key,
                    )?;
                    ticket::WorkspaceTagMutation {
                        operation_key: input.operation_key,
                        action: ticket::WorkspaceTagAction::Delete {
                            tag_id: input.tag_id,
                            expected_last_operation_key: input.expected_last_operation_key,
                        },
                    }
                }
                _ => unreachable!(),
            };
            required("operation_key", &request.operation_key)?;
            json!(
                backend
                    .mutate_workspace_tag(request)
                    .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?
            )
        }
    };
    Ok(ToolOutput {
        summary: kind.name().into(),
        content: Some(content.to_string()),
        attachments: Vec::new(),
    })
}

pub(super) fn definition(client: Arc<dyn WorkspaceClient>, kind: TagToolKind) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(kind.name())
                .description(description(kind.name()))
                .input_schema(kind.schema()),
            Arc::new(TagTool {
                client: client.clone(),
                kind,
            }) as Arc<dyn Tool>,
        )
    })
}

#[cfg(test)]
mod native_command_tests {
    use super::*;
    use assignment_tracking::authorization_tests::{AuthorizedClient, ticket};
    use server_api::authorization::PermissionResource;

    #[tokio::test]
    async fn internal_tag_commands_preserve_resource_authorization_cas_and_replay_requests() {
        let tag = json!({"workspace_id":"workspace", "id":"tag-1", "name":"Name", "last_operation_key":"observed", "deleted":false});
        for (kind, operation, input, response, resource) in [
            (
                TagToolKind::List,
                "ticket.tag.list",
                json!({"limit":2,"after":"tag/0?"}),
                json!([tag.clone()]),
                PermissionResource::Workspace {
                    workspace_id: "workspace".into(),
                },
            ),
            (
                TagToolKind::Create,
                "ticket.tag.create",
                json!({"name":"Name","operation_key":"stable"}),
                tag.clone(),
                PermissionResource::Workspace {
                    workspace_id: "workspace".into(),
                },
            ),
            (
                TagToolKind::Rename,
                "ticket.tag.rename",
                json!({"tag_id":"tag-1","name":"Name","expected_last_operation_key":"observed","operation_key":"stable"}),
                tag.clone(),
                PermissionResource::Workspace {
                    workspace_id: "workspace".into(),
                },
            ),
            (
                TagToolKind::Delete,
                "ticket.tag.delete",
                json!({"tag_id":"tag-1","expected_last_operation_key":"observed","operation_key":"stable"}),
                tag.clone(),
                PermissionResource::Workspace {
                    workspace_id: "workspace".into(),
                },
            ),
            (
                TagToolKind::Set,
                "ticket.tags.set",
                json!({"ticket":"T-A","expected_tag_ids":["tag-0"],"tag_ids":["tag-1"],"operation_key":"stable"}),
                json!([tag]),
                ticket("T-A"),
            ),
        ] {
            let client = AuthorizedClient::new(response);
            assert!(
                execute_native(
                    client.clone(),
                    kind.name(),
                    input.clone(),
                    ToolExecutionContext::direct()
                )
                .await
                .unwrap()
                .is_err()
            );
            assert!(client.requests.lock().unwrap().is_empty());
            client.grant(operation, resource);
            let (_, tool) = definition(client.clone(), kind)();
            let ordinary = tool
                .execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .unwrap();
            let native = execute_native(
                client.clone(),
                kind.name(),
                input.clone(),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(native.content, ordinary.content);
            {
                let requests = client.requests.lock().unwrap();
                assert_eq!(requests[0], requests[1]);
            }
            client.grants.lock().unwrap().clear();
            assert!(
                execute_native(
                    client.clone(),
                    kind.name(),
                    input,
                    ToolExecutionContext::direct()
                )
                .await
                .unwrap()
                .is_err()
            );
            assert_eq!(client.requests.lock().unwrap().len(), 2);
            assert!(
                execute_native(
                    client,
                    "Unknown",
                    Value::Null,
                    ToolExecutionContext::direct()
                )
                .await
                .is_none()
            );
        }
    }

    #[tokio::test]
    async fn internal_tag_commands_apply_business_validation_before_dispatch() {
        let client = AuthorizedClient::new(json!({}));
        for (name, input) in [
            (
                "WorkspaceTagCreate",
                json!({"name":" ","operation_key":"stable"}),
            ),
            ("WorkspaceTagList", json!({"limit":201})),
            (
                "TicketTagsSet",
                json!({"ticket":"T-A","expected_tag_ids":[],"tag_ids":["duplicate","duplicate"],"operation_key":"stable"}),
            ),
            (
                "WorkspaceTagRename",
                json!({"tag_id":"tag-1","name":"Name","operation_key":"stable"}),
            ),
        ] {
            assert!(matches!(
                execute_native(client.clone(), name, input, ToolExecutionContext::direct())
                    .await
                    .unwrap(),
                Err(ToolError::InvalidArgument(_))
            ));
        }
        assert!(client.requests.lock().unwrap().is_empty());
        assert!(client.authorizations.lock().unwrap().is_empty());
    }
}
