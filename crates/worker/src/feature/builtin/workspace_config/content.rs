//! Request-time logical subtree. Metadata is captured per resolution, never
//! cached as authority; authorization and canonical tree CAS stay in Backend.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use server_api::{
    ConfigCommitRequest, ConfigContentType, ConfigTreeChange, WorkspaceConfigAccess,
    WorkspaceConfigAttachment, WorkspaceConfigCommitRequest, WorkspaceConfigCommitResponse,
    WorkspaceConfigNode, WorkspaceConfigNodeKind, WorkspaceConfigObserveRequest,
    WorkspaceConfigObserveResponse, WorkspaceConfigReadRequest, WorkspaceConfigReadResponse,
};
use sha2::{Digest, Sha256};
use wip_protocol::{
    Documentation, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, TypeExpr, Value,
};

use super::backend::{
    MAX_DEPTH, MAX_NODES, MAX_PATH_BYTES, MAX_TEXT_BYTES, WorkspaceConfigBackend, failure, unknown,
};
use super::{CONTENT_ROOT, WorkspaceConfigFeature};
use crate::wip::{
    WipCallContext, WipOperationError, WipOperationHandler, WipOperationOutput, WipProjection,
    WipProjectionKind, WipSubtreeProvider, contextual_reference, json_to_wip,
};

pub(super) struct ConfigProvider {
    pub feature: WorkspaceConfigFeature,
    pub backend: Arc<WorkspaceConfigBackend>,
}

/// Backend tree root is empty; source names are canonical relative config paths.
fn source_path(route: &str) -> Result<String, WipOperationError> {
    if route == CONTENT_ROOT {
        return Ok(String::new());
    }
    let path = route
        .strip_prefix(&format!("{CONTENT_ROOT}/"))
        .ok_or_else(invalid)?;
    validate_source(path)?;
    Ok(path.into())
}
fn validate_source(path: &str) -> Result<(), WipOperationError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(invalid());
    }
    Ok(())
}
fn route(path: &str) -> String {
    if path.is_empty() {
        CONTENT_ROOT.into()
    } else {
        format!("{CONTENT_ROOT}/{path}")
    }
}
fn invalid() -> WipOperationError {
    failure(
        ProtocolErrorCode::InvalidArguments,
        "Invalid Workspace config operation arguments",
    )
}
fn protocol(error: WipOperationError) -> ProtocolError {
    match error {
        WipOperationError::Protocol(error) => error,
        _ => ProtocolError {
            code: ProtocolErrorCode::Internal,
            message: "Workspace config metadata unavailable".into(),
        },
    }
}
/// A denied namespace is absent from observation, not a failure of the whole
/// Worldspace. Do not mask transport, validation or Backend failures as absence.
fn visible_metadata<T>(result: Result<T, WipOperationError>) -> Result<Option<T>, ProtocolError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(WipOperationError::Protocol(error))
            if error.code == ProtocolErrorCode::PermissionDenied =>
        {
            Ok(None)
        }
        Err(error) => Err(protocol(error)),
    }
}

fn same_snapshot(
    snapshot: &WorkspaceConfigObserveResponse,
    other: &WorkspaceConfigObserveResponse,
) -> Result<(), WipOperationError> {
    if snapshot.connection_id != other.connection_id
        || snapshot.validator != other.validator
        || snapshot.revision != other.revision
        || snapshot.digest != other.digest
        || snapshot.entrypoints != other.entrypoints
    {
        return Err(failure(
            ProtocolErrorCode::ValidatorMismatch,
            "Workspace config observation is stale",
        ));
    }
    Ok(())
}

async fn observe(
    backend: &WorkspaceConfigBackend,
    connection: &str,
    paths: Vec<String>,
    depth: u32,
) -> Result<WorkspaceConfigObserveResponse, WipOperationError> {
    if paths.len() > MAX_NODES || depth > MAX_DEPTH {
        return Err(invalid());
    }
    for path in &paths {
        if !path.is_empty() {
            validate_source(path)?;
        }
    }
    let result: WorkspaceConfigObserveResponse = backend
        .post(
            "/observe",
            &WorkspaceConfigObserveRequest {
                connection_id: connection.into(),
                paths: paths.clone(),
                depth,
            },
            false,
        )
        .await?;
    if result.connection_id != connection
        || result.nodes.len() > MAX_NODES
        || result.validator.is_empty()
        || result.validator.len() > 4096
        || result.digest.is_empty()
    {
        return Err(failure(
            ProtocolErrorCode::Internal,
            "Invalid Workspace config metadata",
        ));
    }
    if result.entrypoints.is_empty() || result.entrypoints.len() > MAX_NODES {
        return Err(failure(
            ProtocolErrorCode::Internal,
            "Invalid Workspace config entrypoints",
        ));
    }
    for path in &result.entrypoints {
        validate_source(path)?;
    }
    let mut seen = BTreeSet::new();
    for node in &result.nodes {
        if !node.path.is_empty() {
            validate_source(&node.path)?;
        }
        if !seen.insert(&node.path)
            || node.validator.is_empty()
            || node.validator.len() > 4096
            || !paths.iter().any(|parent| {
                if node.path == *parent {
                    return true;
                }
                let suffix = if parent.is_empty() {
                    Some(node.path.as_str())
                } else {
                    node.path.strip_prefix(&format!("{parent}/"))
                };
                suffix.is_some_and(|s| s.split('/').count() <= depth as usize)
            })
        {
            return Err(failure(
                ProtocolErrorCode::Internal,
                "Invalid Workspace config metadata scope",
            ));
        }
    }
    Ok(result)
}

#[async_trait]
impl WipSubtreeProvider for ConfigProvider {
    fn max_depth(&self) -> u32 {
        MAX_DEPTH
    }
    fn max_nodes(&self) -> usize {
        MAX_NODES
    }

    async fn projection(&self, path: &str) -> Result<Option<WipProjection>, ProtocolError> {
        let source = source_path(path).map_err(protocol)?;
        let Some(attachment) = visible_metadata(self.backend.current_async().await)? else {
            return Ok(None);
        };
        let Some(attachment) = attachment else {
            // An authorized but unattached Worker sees only the self-attach entrance.
            if path != CONTENT_ROOT {
                return Ok(None);
            }
            let mut projection = super::wip::attach_projection(&self.feature);
            projection.interface = contextual_reference(&projection.interface, path);
            projection.object.interfaces = vec![projection.interface.clone()];
            return Ok(Some(projection));
        };
        let Some(snapshot) = visible_metadata(
            observe(
                &self.backend,
                &attachment.connection_id,
                vec![source.clone()],
                0,
            )
            .await,
        )?
        else {
            return Ok(None);
        };
        let Some(node) = snapshot.nodes.iter().find(|n| n.path == source).cloned() else {
            return Ok(None);
        };
        let operations = operations(&node, attachment.access, path == CONTENT_ROOT);
        // Missing nodes are explicit absence observations for safe create, not files.
        if operations.is_empty() && node.kind == WorkspaceConfigNodeKind::Missing {
            return Ok(None);
        }
        let descriptor = descriptor(&operations);
        let interface = contextual_reference("yoi.workspace-config/node/v1", path);
        let interface_validator = Some(Sha256::digest(format!("{descriptor:?}")).to_vec());
        Ok(Some(WipProjection {
            route: path.into(),
            capability: "workspace-config:content".into(),
            kind: WipProjectionKind::Native,
            object: Object {
                name: path.rsplit('/').next().unwrap_or("workspace-config").into(),
                description: Some(format!(
                    "Logical Workspace config {:?}; bodies are available only through Operations",
                    node.kind
                )),
                interfaces: vec![interface.clone()],
                r#ref: None,
                validator: Some(node.validator.as_bytes().to_vec()),
            },
            interface,
            descriptor,
            interface_validator,
            handler: Arc::new(ConfigHandler {
                feature: self.feature.clone(),
                backend: self.backend.clone(),
                attachment,
                snapshot,
                node,
                operations,
            }),
        }))
    }

    async fn children(&self, path: &str) -> Result<Vec<String>, ProtocolError> {
        let source = source_path(path).map_err(protocol)?;
        let Some(Some(attachment)) = visible_metadata(self.backend.current_async().await)? else {
            return Ok(Vec::new());
        };
        let Some(snapshot) = visible_metadata(
            observe(
                &self.backend,
                &attachment.connection_id,
                vec![source.clone()],
                1,
            )
            .await,
        )?
        else {
            return Ok(Vec::new());
        };
        Ok(snapshot
            .nodes
            .iter()
            .filter(|n| n.path != source && n.kind != WorkspaceConfigNodeKind::Missing)
            .map(|n| route(&n.path))
            .collect())
    }
}

fn operations(
    node: &WorkspaceConfigNode,
    access: WorkspaceConfigAccess,
    root: bool,
) -> Vec<String> {
    let mut names = Vec::new();
    // Backend operations are a publication ceiling; never publish a write on RO.
    for name in ["read", "write", "edit", "create", "delete", "apply_changes"] {
        let kind_ok = match name {
            "read" | "write" | "edit" | "delete" => node.kind == WorkspaceConfigNodeKind::File,
            "apply_changes" => root,
            "create" => node.kind != WorkspaceConfigNodeKind::File,
            _ => false,
        };
        if kind_ok
            && (name == "read" || access == WorkspaceConfigAccess::ReadWrite)
            && node.operations.iter().any(|n| n == name)
        {
            names.push(name.into());
        }
    }
    if root {
        names.push("attach".into());
    }
    names
}

fn parameter(name: &str, required: bool, r#type: TypeExpr) -> ParameterDeclaration {
    ParameterDeclaration {
        name: name.into(),
        required,
        documentation: None,
        r#type,
    }
}
fn descriptor(names: &[String]) -> InterfaceDescriptor {
    InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(), types: Vec::new(),
        documentation: Some(Documentation {
            summary: "Logical Workspace configuration file operations".into(),
            details: Some("Backend grants, semantic validation, atomic whole-tree CAS and activation apply. Validators and change digests are managed automatically. apply_changes atomically accepts create/update/delete/rename changes without validators or expected_digest fields. Saving updates configuration for future operation boundaries, not already-running Worker manifests. No OS or command access.".into()),
        }),
        operations: names.iter().map(|name| OperationDeclaration {
            name: name.clone(), documentation: None,
            parameters: match name.as_str() {
                "attach" => vec![parameter("access", false, TypeExpr::String)],
                "read" | "delete" => vec![],
                "write" => vec![parameter("content", true, TypeExpr::String)],
                "edit" => vec![parameter("old_string", true, TypeExpr::String), parameter("new_string", true, TypeExpr::String), parameter("replace_all", false, TypeExpr::Boolean)],
                "create" => vec![parameter("path", false, TypeExpr::String), parameter("content", true, TypeExpr::String), parameter("content_type", false, TypeExpr::String)],
                "apply_changes" => vec![parameter("changes", true, TypeExpr::Json)],
                _ => vec![],
            },
            returns: ReturnDeclaration { documentation: None, r#type: TypeExpr::Json },
        }).collect(),
    }
}

struct ConfigHandler {
    feature: WorkspaceConfigFeature,
    backend: Arc<WorkspaceConfigBackend>,
    attachment: WorkspaceConfigAttachment,
    snapshot: WorkspaceConfigObserveResponse,
    node: WorkspaceConfigNode,
    operations: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Change {
    Create {
        path: String,
        content: String,
        #[serde(default)]
        content_type: Option<ConfigContentType>,
    },
    Update {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
}
impl Change {
    fn target(&self) -> &str {
        match self {
            Self::Create { path, .. } | Self::Update { path, .. } | Self::Delete { path } => path,
            Self::Rename { from, .. } => from,
        }
    }
}
fn text(arguments: &BTreeMap<String, Value>, key: &str) -> Result<String, WipOperationError> {
    match arguments.get(key) {
        Some(Value::String(s)) if s.len() <= MAX_TEXT_BYTES => Ok(s.clone()),
        _ => Err(invalid()),
    }
}
fn content_type(value: Option<&Value>) -> Result<ConfigContentType, WipOperationError> {
    match value {
        None => Ok(ConfigContentType::Decodal),
        Some(Value::String(value)) if value == "decodal" => Ok(ConfigContentType::Decodal),
        Some(Value::String(value)) if value == "text" => Ok(ConfigContentType::Text),
        _ => Err(invalid()),
    }
}

impl ConfigHandler {
    async fn read(&self) -> Result<WorkspaceConfigReadResponse, WipOperationError> {
        let response: WorkspaceConfigReadResponse = self
            .backend
            .post(
                "/read",
                &WorkspaceConfigReadRequest {
                    connection_id: self.attachment.connection_id.clone(),
                    path: self.node.path.clone(),
                    validator: self.node.validator.clone(),
                },
                false,
            )
            .await?;
        if response.path != self.node.path
            || response.validator != self.node.validator
            || Some(&response.digest) != self.node.digest.as_ref()
            || Some(&response.content_type) != self.node.content_type.as_ref()
            || response.content.len() > MAX_TEXT_BYTES
        {
            return Err(failure(
                ProtocolErrorCode::Internal,
                "Invalid Workspace config read response",
            ));
        }
        Ok(response)
    }

    async fn canonical_changes(
        &self,
        changes: Vec<Change>,
    ) -> Result<Vec<ConfigTreeChange>, WipOperationError> {
        if changes.is_empty() || changes.len() > MAX_NODES {
            return Err(invalid());
        }
        let mut paths = BTreeSet::new();
        let mut text_bytes = 0usize;
        for change in &changes {
            validate_source(change.target())?;
            if !paths.insert(change.target().to_owned()) {
                return Err(invalid());
            }
            match change {
                Change::Create { content, .. } | Change::Update { content, .. } => {
                    text_bytes = text_bytes.saturating_add(content.len());
                }
                Change::Rename { to, .. } => {
                    validate_source(to)?;
                }
                _ => {}
            }
        }
        if text_bytes > MAX_TEXT_BYTES {
            return Err(failure(
                ProtocolErrorCode::ResourceLimitExceeded,
                "Workspace config text exceeds limit",
            ));
        }
        // Resolve canonical per-file preconditions at the SAME captured tree revision.
        let metadata = observe(
            &self.backend,
            &self.attachment.connection_id,
            paths.into_iter().collect(),
            0,
        )
        .await?;
        same_snapshot(&self.snapshot, &metadata)?;
        changes
            .into_iter()
            .map(|change| {
                let node = metadata
                    .nodes
                    .iter()
                    .find(|n| n.path == change.target())
                    .ok_or_else(invalid)?;
                let digest = || {
                    node.digest
                        .clone()
                        .filter(|_| node.kind == WorkspaceConfigNodeKind::File)
                        .ok_or_else(|| {
                            failure(
                                ProtocolErrorCode::ValidatorMismatch,
                                "Workspace config file no longer exists",
                            )
                        })
                };
                Ok(match change {
                    Change::Create {
                        path,
                        content,
                        content_type,
                    } => {
                        if node.kind != WorkspaceConfigNodeKind::Missing {
                            return Err(failure(
                                ProtocolErrorCode::ValidatorMismatch,
                                "Workspace config create target already exists",
                            ));
                        }
                        ConfigTreeChange::Create {
                            path,
                            content,
                            content_type: content_type.unwrap_or(ConfigContentType::Decodal),
                        }
                    }
                    Change::Update { path, content } => ConfigTreeChange::Update {
                        path,
                        content,
                        expected_digest: digest()?,
                    },
                    Change::Delete { path } => ConfigTreeChange::Delete {
                        path,
                        expected_digest: digest()?,
                    },
                    Change::Rename { from, to } => ConfigTreeChange::Rename {
                        from,
                        to,
                        expected_digest: digest()?,
                    },
                })
            })
            .collect()
    }
}

#[async_trait]
impl WipOperationHandler for ConfigHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let dispatched = std::sync::atomic::AtomicBool::new(false);
        tokio::time::timeout(
            super::DEADLINE,
            self.execute(operation, arguments, context, &dispatched),
        )
        .await
        .map_err(|_| {
            if dispatched.load(std::sync::atomic::Ordering::Acquire) {
                unknown()
            } else {
                failure(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "Workspace config operation deadline exceeded before effects",
                )
            }
        })?
    }
}

impl ConfigHandler {
    async fn execute(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
        dispatched: &std::sync::atomic::AtomicBool,
    ) -> Result<WipOperationOutput, WipOperationError> {
        if !self.operations.iter().any(|name| name == operation) {
            return Err(failure(
                ProtocolErrorCode::OperationNotFound,
                "Workspace config operation is not published",
            ));
        }
        // Direct handler calls also enforce parameter shape; no model validators accepted.
        let declaration = descriptor(&self.operations)
            .operations
            .into_iter()
            .find(|op| op.name == operation)
            .ok_or_else(invalid)?;
        if arguments
            .keys()
            .any(|name| !declaration.parameters.iter().any(|p| p.name == *name))
        {
            return Err(invalid());
        }
        if operation == "attach" {
            dispatched.store(true, std::sync::atomic::Ordering::Release);
            return super::wip::attach_projection(&self.feature)
                .handler
                .call(operation, arguments, context)
                .await;
        }
        if operation == "read" {
            let result = self.read().await?;
            // Validator goes to Client state, not the model operation result.
            let value = json_to_wip(&json!({
                "path": result.path, "content": result.content,
                "content_type": result.content_type, "digest": result.digest,
            }))
            .map_err(|_| invalid())?;
            return Ok(WipOperationOutput::native_with_validator(
                value,
                self.node.validator.as_bytes().to_vec(),
            ));
        }
        if self.attachment.access != WorkspaceConfigAccess::ReadWrite {
            return Err(failure(
                ProtocolErrorCode::PermissionDenied,
                "Workspace config write denied",
            ));
        }
        let changes = match operation {
            "write" => vec![Change::Update {
                path: self.node.path.clone(),
                content: text(arguments, "content")?,
            }],
            "edit" => {
                let original = self.read().await?.content;
                let old = text(arguments, "old_string")?;
                let new = text(arguments, "new_string")?;
                let all = match arguments.get("replace_all") {
                    None => false,
                    Some(Value::Boolean(all)) => *all,
                    _ => return Err(invalid()),
                };
                if old.is_empty() {
                    return Err(invalid());
                }
                let count = original.matches(&old).count();
                if count == 0 || (!all && count != 1) {
                    return Err(invalid());
                }
                let count = if all { count } else { 1 };
                let size = original
                    .len()
                    .checked_sub(count.saturating_mul(old.len()))
                    .and_then(|n| n.checked_add(count.checked_mul(new.len())?))
                    .filter(|n| *n <= MAX_TEXT_BYTES);
                if size.is_none() {
                    return Err(failure(
                        ProtocolErrorCode::ResourceLimitExceeded,
                        "Workspace config edit exceeds text limit",
                    ));
                }
                let content = if all {
                    original.replace(&old, &new)
                } else {
                    original.replacen(&old, &new, 1)
                };
                vec![Change::Update {
                    path: self.node.path.clone(),
                    content,
                }]
            }
            "delete" => vec![Change::Delete {
                path: self.node.path.clone(),
            }],
            "create" => {
                let path = match (self.node.kind, arguments.get("path")) {
                    (WorkspaceConfigNodeKind::Missing, None) => self.node.path.clone(),
                    (WorkspaceConfigNodeKind::Directory, Some(Value::String(path))) => {
                        validate_source(path)?;
                        if self.node.path.is_empty() {
                            path.clone()
                        } else {
                            format!("{}/{path}", self.node.path)
                        }
                    }
                    _ => return Err(invalid()),
                };
                vec![Change::Create {
                    path,
                    content: text(arguments, "content")?,
                    content_type: Some(content_type(arguments.get("content_type"))?),
                }]
            }
            "apply_changes" => {
                let Some(value) = arguments.get("changes") else {
                    return Err(invalid());
                };
                serde_json::from_value::<Vec<Change>>(
                    crate::wip::wip_to_json(value).map_err(|_| invalid())?,
                )
                .map_err(|_| invalid())?
            }
            _ => return Err(invalid()),
        };
        let changes = self.canonical_changes(changes).await?;
        dispatched.store(true, std::sync::atomic::Ordering::Release);
        let result: WorkspaceConfigCommitResponse = self
            .backend
            .post(
                "/commit",
                &WorkspaceConfigCommitRequest {
                    connection_id: self.attachment.connection_id.clone(),
                    validator: self.snapshot.validator.clone(),
                    request: ConfigCommitRequest {
                        base_revision: self.snapshot.revision,
                        base_digest: self.snapshot.digest.clone(),
                        changes,
                        entrypoints: self.snapshot.entrypoints.clone(),
                    },
                },
                true,
            )
            .await?;
        // Commit response supplies ROOT state. Obtain the exact post-commit PATH
        // validator, never substitute the root validator for a file validator.
        let updated = observe(
            &self.backend,
            &self.attachment.connection_id,
            vec![self.node.path.clone()],
            0,
        )
        .await
        .map_err(|_| unknown())?;
        if updated.revision != result.revision
            || updated.digest != result.digest
            || updated.validator != result.validator
        {
            return Err(unknown());
        }
        let node = updated
            .nodes
            .iter()
            .find(|n| n.path == self.node.path)
            .ok_or_else(unknown)?;
        let value = json_to_wip(&json!({"revision": result.revision, "digest": result.digest,
            "activation": "Saved to canonical Workspace configuration; existing Worker manifests are not rewritten"})).map_err(|_| unknown())?;
        Ok(WipOperationOutput::native_with_validator(
            value,
            node.validator.as_bytes().to_vec(),
        ))
    }
}
