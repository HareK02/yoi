//! Host-owned Repository, Workdir and Worker attachment Objects.
//! Inventory is resolved from Workspace authority on demand, not a second catalog.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use manifest::{ToolPermissionAction, ToolPermissionConfig};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use wip_protocol::{
    Documentation, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, TypeExpr, Value,
};

use super::*;
use crate::permission::permission_action_for;
use crate::wip::{
    WipCallContext, WipDynamicItem, WipDynamicItemResolver, WipDynamicMount,
    WipDynamicOperationContribution, WipDynamicOperationResolver, WipMountError, WipMountRegistry,
    WipOperationContribution, WipOperationError, WipOperationHandler, WipOperationOutput,
    WipProjection, WipProjectionKind, json_to_wip, wip_to_json,
};

const REPOSITORIES: &str = "/repositories";
const WORKDIRS: &str = "/workdirs";
const ATTACHMENTS: &str = "/workdir-attachments";

#[derive(Clone, Copy, Debug)]
enum Domain {
    Repository,
    Workdir,
    Attachment,
}

impl Domain {
    fn route(self) -> &'static str {
        match self {
            Self::Repository => REPOSITORIES,
            Self::Workdir => WORKDIRS,
            Self::Attachment => ATTACHMENTS,
        }
    }
    fn owner(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Workdir => "workdir",
            Self::Attachment => "workdir-attachment",
        }
    }
    fn list_permission(self) -> &'static str {
        match self {
            Self::Repository => "RepositoryList",
            Self::Workdir => LIST_TOOL,
            Self::Attachment => "WorkdirAttachmentList",
        }
    }
    fn read_permission(self) -> &'static str {
        match self {
            Self::Repository => "RepositoryRead",
            Self::Workdir => "WorkdirRead",
            Self::Attachment => "WorkdirAttachmentRead",
        }
    }
    fn identity_field(self) -> &'static str {
        match self {
            Self::Repository => "repository_key",
            Self::Workdir => "working_directory_id",
            Self::Attachment => "connection_id",
        }
    }
}

#[derive(Clone)]
struct Provider {
    backend: WorkspaceHttpWorkdirBackend,
    permissions: Option<ToolPermissionConfig>,
    catalog: bool,
    manage: bool,
    config_content: bool,
}

/// Register Object ownership once, then contribute independent management operations.
/// The passed Feature shares its session router, mutation lock and child lifecycle with Tools.
pub fn mount_workspace_workdir_wip(
    registry: &mut WipMountRegistry,
    feature: &ManageWorkdirFeature,
    catalog: bool,
    manage: bool,
    permissions: Option<ToolPermissionConfig>,
) -> Result<(), WipMountError> {
    let provider = Provider {
        backend: feature.backend(),
        permissions,
        catalog,
        manage,
        config_content: registry
            .routes()
            .any(|route| route == crate::feature::builtin::workspace_config::CONTENT_ROOT),
    };
    for domain in [Domain::Repository, Domain::Workdir, Domain::Attachment] {
        let namespace =
            registry.allocate_namespace(domain.owner(), domain.route().trim_start_matches('/'))?;
        let interface = format!("yoi.{}/collection/v1", domain.owner());
        let handler = Arc::new(CollectionHandler {
            provider: provider.clone(),
            domain,
        });
        registry.mount(WipProjection {
            route: namespace.root().into(),
            capability: format!("{}:collection", domain.owner()),
            kind: WipProjectionKind::Native,
            object: object(
                domain.route(),
                domain.route(),
                &interface,
                &json!({"kind": "collection"}),
            ),
            interface: interface.clone(),
            descriptor: collection_descriptor(domain, false),
            interface_validator: None,
            handler: handler.clone(),
        })?;
        if manage && matches!(domain, Domain::Workdir) {
            registry.contribute_operations(WipOperationContribution {
                route: domain.route().into(),
                contributor: FEATURE_ID.into(),
                interface,
                descriptor: collection_descriptor(domain, true),
                handler,
            })?;
        }
        let item_interface = format!("yoi.{}/item/v1", domain.owner());
        let resolver = Arc::new(ItemResolver {
            provider: provider.clone(),
            domain,
        });
        registry.mount_dynamic(WipDynamicMount {
            collection_route: domain.route().into(),
            capability: format!("{}:item", domain.owner()),
            interface: item_interface.clone(),
            descriptor: item_descriptor(domain, false),
            interface_validator: None,
            resolver: resolver.clone(),
        })?;
        if manage && !matches!(domain, Domain::Repository) {
            registry.contribute_dynamic_operations(WipDynamicOperationContribution {
                collection_route: domain.route().into(),
                contributor: FEATURE_ID.into(),
                interface: item_interface,
                descriptor: item_descriptor(domain, true),
                resolver,
            })?;
        }
    }
    registry.replace_compatibility_tools(
        WORKDIRS,
        [LIST_TOOL, CREATE_TOOL, ATTACH_TOOL, DELETE_TOOL],
    )?;
    registry.replace_compatibility_tools(ATTACHMENTS, [DETACH_TOOL])?;
    Ok(())
}

impl Provider {
    fn allowed(&self, name: &str, input: &Json) -> bool {
        self.permissions
            .as_ref()
            .is_none_or(|p| permission_action_for(p, name, input) == ToolPermissionAction::Allow)
    }
    // An argument-dependent permission cannot be decided until call. Advertise only
    // if the policy can allow this tool, then recheck the exact route-bound input.
    fn may_allow(&self, name: &str) -> bool {
        let Some(p) = &self.permissions else {
            return true;
        };
        for rule in p.rules.iter().filter(|r| r.tool.eq_ignore_ascii_case(name)) {
            if rule.action == ToolPermissionAction::Allow {
                return true;
            }
            if rule.pattern == "*" {
                return false;
            }
        }
        p.default_action == ToolPermissionAction::Allow
    }
    fn authorize(&self, name: &str, input: &Json) -> Result<(), WipOperationError> {
        if self.allowed(name, input) {
            Ok(())
        } else {
            Err(failure(
                ProtocolErrorCode::PermissionDenied,
                format!("permission denied for `{name}`; approval-required policies fail closed"),
            ))
        }
    }
    fn request(&self, path: &str) -> Result<Json, WipOperationError> {
        let workspace = self.backend.workspace_id().map_err(map_tool_error)?;
        let response = self
            .backend
            .client
            .execute(WorkspaceRequest::get(format!(
                "/api/w/{}/{path}",
                encode_path_segment(workspace)
            )))
            .map_err(|_| {
                WipOperationError::OutcomeUnknown("Workspace inventory request unavailable".into())
            })?;
        if !response.is_success() {
            return Err(match response.status {
                401 | 403 => failure(
                    ProtocolErrorCode::PermissionDenied,
                    "Workspace authority denied this inventory request",
                ),
                404 => failure(
                    ProtocolErrorCode::NotFound,
                    "inventory Object no longer exists",
                ),
                409 => failure(
                    ProtocolErrorCode::ValidatorMismatch,
                    "inventory observation is stale; rediscover",
                ),
                _ => WipOperationError::OutcomeUnknown("Workspace inventory request failed".into()),
            });
        }
        serde_json::from_str(&response.body).map_err(|_| {
            WipOperationError::OutcomeUnknown("invalid Workspace inventory response".into())
        })
    }
    fn inventory(&self, domain: Domain) -> Result<Vec<Json>, WipOperationError> {
        let response = self.request(match domain {
            Domain::Repository => "repositories",
            Domain::Workdir => "workers/self/workdir-catalog?limit=1",
            Domain::Attachment => "workers/self/workdir-attachments?limit=1",
        })?;
        response
            .get("items")
            .and_then(Json::as_array)
            .cloned()
            .ok_or_else(|| {
                WipOperationError::OutcomeUnknown("invalid Workspace inventory collection".into())
            })
    }
    fn raw_item(&self, domain: Domain, id: &str) -> Result<Json, WipOperationError> {
        match domain {
            Domain::Attachment => self
                .request(&format!(
                    "workers/self/workdir-attachments?limit=1&connection_id={}",
                    encode_path_segment(id)
                ))?
                .get("items")
                .and_then(Json::as_array)
                .cloned()
                .ok_or_else(|| {
                    WipOperationError::OutcomeUnknown("invalid attachment lookup response".into())
                })?
                .into_iter()
                .find(|v| v.get("connection_id").and_then(Json::as_str) == Some(id))
                .ok_or_else(|| {
                    failure(
                        ProtocolErrorCode::NotFound,
                        "attachment lifetime no longer exists; list current attachments",
                    )
                }),
            Domain::Repository | Domain::Workdir => {
                let prefix = if matches!(domain, Domain::Repository) {
                    "repositories"
                } else {
                    "working-directories"
                };
                self.request(&format!("{prefix}/{}", encode_path_segment(id)))?
                    .get("item")
                    .cloned()
                    .ok_or_else(|| {
                        WipOperationError::OutcomeUnknown("invalid Workspace inventory item".into())
                    })
            }
        }
    }
    fn read_input(&self, domain: Domain, id: &str) -> Json {
        json!({domain.identity_field(): id})
    }
    fn projected(&self, domain: Domain, raw: &Json) -> Result<Json, WipOperationError> {
        let id = raw
            .get(domain.identity_field())
            .and_then(Json::as_str)
            .ok_or_else(|| {
                WipOperationError::OutcomeUnknown("inventory identity missing".into())
            })?;
        let mut result = serde_json::Map::new();
        let fields: &[&str] = match domain {
            Domain::Repository => &[
                "repository_key",
                "kind",
                "provider",
                "display_name",
                "description",
                "default_selector",
            ],
            Domain::Workdir => &[
                "working_directory_id",
                "display_name",
                "creation_selector",
                "creation_ref",
                "creation_tree",
                "current_selector",
                "current_ref",
                "current_tree",
                "status",
                "cleanliness",
                "occupied_by",
            ],
            Domain::Attachment => &[
                "connection_id",
                "alias",
                "working_directory_id",
                "capabilities",
            ],
        };
        for field in fields {
            if let Some(value) = raw.get(*field) {
                result.insert((*field).into(), value.clone());
            }
        }
        if matches!(domain, Domain::Repository) {
            // A missing default is explicit. No guessed main/ref, source URI or credentials.
            result.entry("default_selector").or_insert(Json::Null);
        }
        if matches!(domain, Domain::Workdir) {
            if let Some(source) = raw.get("source") {
                let kind = source
                    .get("kind")
                    .and_then(Json::as_str)
                    .unwrap_or("unknown");
                let mut projected = json!({"kind": kind});
                if kind == "repository"
                    && let Some(key) = source.get("repository_key")
                {
                    projected["repository_key"] = key.clone();
                    projected["repository_path"] = json!(item_path(
                        Domain::Repository,
                        key.as_str().unwrap_or_default()
                    ));
                }
                if kind == "workspace_config"
                    && source.get("content_path").and_then(Json::as_str)
                        == Some(crate::feature::builtin::workspace_config::CONTENT_ROOT)
                    && matches!(
                        source.get("access").and_then(Json::as_str),
                        Some("read_only" | "read_write")
                    )
                {
                    for field in ["access", "content_path", "purpose"] {
                        if let Some(value) = source.get(field) {
                            projected[field] = value.clone();
                        }
                    }
                }
                result.insert("source".into(), projected);
            }
        }
        if matches!(domain, Domain::Attachment) {
            if let Some(id) = raw.get("working_directory_id").and_then(Json::as_str) {
                result.insert("workdir_path".into(), json!(item_path(Domain::Workdir, id)));
            }
            // Content references are resolved from current Backend authority,
            // never inferred from the alias or an OS session. An old lifetime
            // cannot refer to a replacement connection at the same root.
            if self.config_content
                && raw.get("alias").and_then(Json::as_str)
                    == Some(crate::feature::builtin::workspace_config::ATTACHMENT_ALIAS)
                && let Ok(Some(current)) =
                    crate::feature::builtin::workspace_config::backend::WorkspaceConfigBackend::new(
                        self.backend.client.clone(),
                    )
                    .current()
                && current.connection_id == id
                && raw.get("working_directory_id").and_then(Json::as_str)
                    == Some(current.working_directory_id.as_str())
                && raw.get("alias").and_then(Json::as_str) == Some(current.alias.as_str())
            {
                result.insert("content_path".into(), json!(current.content_path));
                result.insert("name".into(), json!(current.name));
                result.insert("purpose".into(), json!(current.purpose));
                result.insert("access".into(), json!(current.access));
            }
            if !result.contains_key("content_path")
                && let Some(alias) = raw.get("alias").and_then(Json::as_str)
                && let Ok(selected) = self.backend.session_router.resolve(Some(alias))
                && raw.get("working_directory_id").and_then(Json::as_str)
                    == Some(selected.session.workdir().id().as_str())
                && selected
                    .session
                    .capabilities()
                    .supports(workdir::WorkdirSessionCapability::Read)
            {
                result.insert(
                    "checkout_path".into(),
                    json!(crate::checkout::checkout_root(alias)),
                );
                result.insert("checkout_slug".into(), json!(encode_identity(alias)));
            }
        }
        result.insert("path".into(), json!(item_path(domain, id)));
        Ok(Json::Object(result))
    }
}

struct CollectionHandler {
    provider: Provider,
    domain: Domain,
}
#[async_trait]
impl WipOperationHandler for CollectionHandler {
    fn contextual_interface(&self) -> bool {
        true
    }
    fn object_validator(&self) -> Option<Vec<u8>> {
        let mut digest = Sha256::new();
        digest.update(self.domain.route());
        if matches!(self.domain, Domain::Workdir | Domain::Attachment) {
            // The complete-set revision is computed by the existing Backend
            // registry/ledger in the same snapshot as a bounded page. Never infer
            // collection freshness from just the first page's items.
            let path = if matches!(self.domain, Domain::Workdir) {
                "workers/self/workdir-catalog?limit=1"
            } else {
                "workers/self/workdir-attachments?limit=1"
            };
            match self
                .provider
                .request(path)
                .ok()
                .and_then(|p| p.get("revision").and_then(Json::as_str).map(str::to_owned))
            {
                Some(revision) => {
                    digest.update([0]);
                    digest.update(revision);
                }
                None => digest.update(b"inventory-revision-unavailable"),
            }
            return Some(digest.finalize().to_vec());
        }
        match self.provider.inventory(self.domain) {
            Ok(items) => {
                for raw in items {
                    if let Some(id) = raw.get(self.domain.identity_field()).and_then(Json::as_str)
                        && self.provider.allowed(
                            self.domain.read_permission(),
                            &self.provider.read_input(self.domain, id),
                        )
                        && let Ok(value) = self.provider.projected(self.domain, &raw)
                    {
                        digest.update(value.to_string());
                        digest.update([0]);
                    }
                }
            }
            Err(_) => digest.update(b"inventory-unavailable"),
        }
        Some(digest.finalize().to_vec())
    }
    fn is_visible(&self) -> bool {
        (self.provider.catalog
            && (self.provider.may_allow(self.domain.list_permission())
                || self.provider.may_allow(self.domain.read_permission())))
            || (self.provider.manage
                && matches!(self.domain, Domain::Workdir)
                && self.provider.may_allow(CREATE_TOOL))
    }
    fn operation_available(&self, operation: &str) -> bool {
        match operation {
            "list" => {
                self.provider.catalog && self.provider.may_allow(self.domain.list_permission())
            }
            "create" => {
                self.provider.manage
                    && matches!(self.domain, Domain::Workdir)
                    && self.provider.may_allow(CREATE_TOOL)
            }
            _ => false,
        }
    }
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let input = argument_json(arguments)?;
        if !self.operation_available(operation) {
            return Err(failure(
                ProtocolErrorCode::PermissionDenied,
                "operation is not available",
            ));
        }
        let value = match operation {
            "list" => {
                self.provider
                    .authorize(self.domain.list_permission(), &input)?;
                let limit = page_limit(&input)?;
                let cursor = input.get("cursor").and_then(Json::as_str);
                if matches!(self.domain, Domain::Workdir | Domain::Attachment) {
                    let attachment = matches!(self.domain, Domain::Attachment);
                    let path = if attachment {
                        let offset = cursor
                            .map(|c| {
                                c.strip_prefix("offset:")
                                    .and_then(|s| s.parse::<u32>().ok())
                                    .ok_or_else(|| {
                                        failure(
                                            ProtocolErrorCode::InvalidArguments,
                                            "invalid attachment page cursor",
                                        )
                                    })
                            })
                            .transpose()?
                            .unwrap_or(0);
                        format!("workers/self/workdir-attachments?limit={limit}&offset={offset}")
                    } else {
                        let suffix = cursor
                            .map(|c| format!("&cursor={}", encode_path_segment(c)))
                            .unwrap_or_default();
                        format!("workers/self/workdir-catalog?limit={limit}{suffix}")
                    };
                    let page = self.provider.request(&path)?;
                    let raws = page
                        .get("items")
                        .and_then(Json::as_array)
                        .filter(|v| v.len() <= limit)
                        .ok_or_else(|| {
                            WipOperationError::OutcomeUnknown(
                                "invalid bounded inventory page".into(),
                            )
                        })?;
                    if page.get("revision").and_then(Json::as_str).is_none() {
                        return Err(WipOperationError::OutcomeUnknown(
                            "inventory page revision missing".into(),
                        ));
                    }
                    let mut items = Vec::new();
                    for raw in raws {
                        let id = raw
                            .get(self.domain.identity_field())
                            .and_then(Json::as_str)
                            .ok_or_else(|| {
                                WipOperationError::OutcomeUnknown(
                                    "inventory identity missing".into(),
                                )
                            })?;
                        if self.provider.allowed(
                            self.domain.read_permission(),
                            &self.provider.read_input(self.domain, id),
                        ) {
                            items.push(self.provider.projected(self.domain, raw)?);
                        }
                    }
                    let next = if attachment {
                        match page.get("next_offset") {
                            Some(Json::Null) => None,
                            Some(v) if v.as_u64().is_some_and(|n| n <= u32::MAX as u64) => {
                                Some(format!("offset:{}", v.as_u64().unwrap()))
                            }
                            _ => {
                                return Err(WipOperationError::OutcomeUnknown(
                                    "invalid attachment next page".into(),
                                ));
                            }
                        }
                    } else {
                        match page.get("next_cursor") {
                            Some(Json::Null) => None,
                            Some(Json::String(v)) if cursor.is_none_or(|old| v.as_str() > old) => {
                                Some(v.clone())
                            }
                            _ => {
                                return Err(WipOperationError::OutcomeUnknown(
                                    "invalid Workdir next page".into(),
                                ));
                            }
                        }
                    };
                    // Empty describes this permission-filtered page, not the
                    // entire catalog. A sparse page still advances the Backend cursor.
                    return native(
                        json!({"empty":items.is_empty(), "items":items, "has_more":next.is_some(), "next_cursor":next, "limit":limit}),
                    );
                }
                let mut items = Vec::new();
                for raw in self.provider.inventory(self.domain)? {
                    let id = raw
                        .get(self.domain.identity_field())
                        .and_then(Json::as_str)
                        .ok_or_else(|| {
                            WipOperationError::OutcomeUnknown("inventory identity missing".into())
                        })?;
                    if self.provider.allowed(
                        self.domain.read_permission(),
                        &self.provider.read_input(self.domain, id),
                    ) {
                        items.push(self.provider.projected(self.domain, &raw)?);
                    }
                }
                items.sort_by(|a, b| {
                    a[self.domain.identity_field()]
                        .as_str()
                        .cmp(&b[self.domain.identity_field()].as_str())
                });
                let total_visible = items.len();
                let remaining: Vec<_> = items
                    .into_iter()
                    .filter(|v| {
                        cursor.is_none_or(|c| {
                            v[self.domain.identity_field()]
                                .as_str()
                                .is_some_and(|id| id > c)
                        })
                    })
                    .collect();
                let has_more = remaining.len() > limit;
                let items: Vec<_> = remaining.into_iter().take(limit).collect();
                let next_cursor = if has_more {
                    items
                        .last()
                        .map(|v| v[self.domain.identity_field()].clone())
                } else {
                    None
                };
                json!({"items": items, "empty": total_visible == 0, "has_more": has_more, "next_cursor": next_cursor, "limit": limit})
            }
            "create" => {
                self.provider.authorize(CREATE_TOOL, &input)?;
                let tool = WorkspaceHttpWorkdirTool {
                    backend: self.provider.backend.clone(),
                    operation: WorkdirOperation::Create,
                };
                let output = tool
                    .execute(&input.to_string(), context.execution)
                    .await
                    .map_err(map_tool_error)?;
                let raw = output_json(output)?;
                json!({"item": self.provider.projected(Domain::Workdir, &raw["item"])?, "runtime_id":raw["runtime_id"]})
            }
            _ => {
                return Err(failure(
                    ProtocolErrorCode::OperationNotFound,
                    "operation not published",
                ));
            }
        };
        native(value)
    }
}

struct ItemResolver {
    provider: Provider,
    domain: Domain,
}
impl WipDynamicItemResolver for ItemResolver {
    fn resolve(&self, reference: &str) -> Option<WipDynamicItem> {
        let id = decode_identity(reference)?;
        if !self.provider.catalog
            || !self.provider.allowed(
                self.domain.read_permission(),
                &self.provider.read_input(self.domain, &id),
            )
        {
            return None;
        }
        let raw = self.provider.raw_item(self.domain, &id).ok()?;
        let projected = self.provider.projected(self.domain, &raw).ok()?;
        let interface = format!("yoi.{}/item/v1", self.domain.owner());
        Some(WipDynamicItem {
            object: object(&item_path(self.domain, &id), &id, &interface, &projected),
            handler: Arc::new(ItemHandler {
                provider: self.provider.clone(),
                domain: self.domain,
                id,
            }),
        })
    }
}
impl WipDynamicOperationResolver for ItemResolver {
    fn handler(&self, reference: &str) -> Arc<dyn WipOperationHandler> {
        Arc::new(ItemHandler {
            provider: self.provider.clone(),
            domain: self.domain,
            id: decode_identity(reference).unwrap_or_default(),
        })
    }
}
struct ItemHandler {
    provider: Provider,
    domain: Domain,
    id: String,
}
impl ItemHandler {
    fn input(&self, arguments: &BTreeMap<String, Value>) -> Result<Json, WipOperationError> {
        let mut input = argument_json(arguments)?;
        // Defense in depth in addition to descriptor validation, including direct handler calls.
        if input.get(self.domain.identity_field()).is_some()
            || input.get("alias").is_some() && matches!(self.domain, Domain::Attachment)
        {
            return Err(failure(
                ProtocolErrorCode::InvalidArguments,
                "route-bound identity cannot be overridden",
            ));
        }
        input[self.domain.identity_field()] = json!(self.id);
        Ok(input)
    }
    fn supports(&self, operation: &str) -> bool {
        let Ok(raw) = self.provider.raw_item(self.domain, &self.id) else {
            return false;
        };
        match operation {
            "read" => true,
            "attach" => {
                matches!(self.domain, Domain::Workdir)
                    && matches!(
                        raw.get("source")
                            .and_then(|v| v.get("kind"))
                            .and_then(Json::as_str),
                        Some("repository" | "external_grant")
                    )
                    && raw.get("status").and_then(Json::as_str) == Some("active")
            }
            "delete" => {
                matches!(self.domain, Domain::Workdir)
                    && raw.get("cleanup_target").is_some_and(|v| !v.is_null())
            }
            "detach" => matches!(self.domain, Domain::Attachment),
            _ => false,
        }
    }
}
#[async_trait]
impl WipOperationHandler for ItemHandler {
    fn contextual_interface(&self) -> bool {
        true
    }
    fn operation_available(&self, operation: &str) -> bool {
        let (enabled, permission) = match operation {
            "read" => (self.provider.catalog, self.domain.read_permission()),
            "attach" => (self.provider.manage, ATTACH_TOOL),
            "delete" => (self.provider.manage, DELETE_TOOL),
            "detach" => (self.provider.manage, DETACH_TOOL),
            _ => return false,
        };
        enabled && self.provider.may_allow(permission) && self.supports(operation)
    }
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let mut input = self.input(arguments)?;
        // Recheck reference permission as well as mutation permission on every call.
        self.provider.authorize(
            self.domain.read_permission(),
            &self.provider.read_input(self.domain, &self.id),
        )?;
        if !self.operation_available(operation) {
            return Err(failure(
                ProtocolErrorCode::OperationNotFound,
                "target or subject does not support this operation",
            ));
        }
        let value = match operation {
            "read" => self
                .provider
                .projected(self.domain, &self.provider.raw_item(self.domain, &self.id)?)?,
            "attach" | "delete" => {
                let (permission, op) = if operation == "attach" {
                    (ATTACH_TOOL, WorkdirOperation::Attach)
                } else {
                    (DELETE_TOOL, WorkdirOperation::Delete)
                };
                self.provider.authorize(permission, &input)?;
                let output = WorkspaceHttpWorkdirTool {
                    backend: self.provider.backend.clone(),
                    operation: op,
                }
                .execute(&input.to_string(), context.execution)
                .await
                .map_err(map_tool_error)?;
                let raw = output_json(output)?;
                if operation == "attach" {
                    let mut result = raw;
                    result["workdir_path"] = json!(item_path(Domain::Workdir, &self.id));
                    result["attachments_path"] = json!(ATTACHMENTS);
                    if let Some(alias) = input.get("alias").and_then(Json::as_str)
                        && self
                            .provider
                            .backend
                            .session_router
                            .resolve(Some(alias))
                            .is_ok()
                    {
                        result["checkout_path"] = json!(crate::checkout::checkout_root(alias));
                        result["checkout_slug"] = json!(encode_identity(alias));
                    }
                    // Backend attachment list supplies the durable lifetime identity.
                    result
                } else {
                    raw
                }
            }
            "detach" => {
                let raw = self.provider.raw_item(Domain::Attachment, &self.id)?;
                let alias = raw.get("alias").and_then(Json::as_str).ok_or_else(|| {
                    WipOperationError::OutcomeUnknown("attachment alias missing".into())
                })?;
                input
                    .as_object_mut()
                    .expect("route input is an object")
                    .remove("connection_id");
                input["alias"] = json!(alias);
                self.provider.authorize(DETACH_TOOL, &input)?;
                output_json(
                    self.provider
                        .backend
                        .detach_managed(alias, Some(&self.id))
                        .await
                        .map_err(map_tool_error)?,
                )?
            }
            _ => {
                return Err(failure(
                    ProtocolErrorCode::OperationNotFound,
                    "operation not published",
                ));
            }
        };
        native(value)
    }
}

fn object(path: &str, identity: &str, interface: &str, value: &Json) -> Object {
    let mut digest = Sha256::new();
    digest.update(path);
    digest.update([0]);
    digest.update(value.to_string());
    Object {
        name: path.rsplit('/').next().unwrap_or_default().into(),
        description: Some(format!("Workspace authority Object: {identity}")),
        interfaces: vec![interface.into()],
        r#ref: Some(identity.into()),
        validator: Some(digest.finalize().to_vec()),
    }
}
fn native(value: Json) -> Result<WipOperationOutput, WipOperationError> {
    Ok(WipOperationOutput::native(
        json_to_wip(&value).map_err(WipOperationError::OutcomeUnknown)?,
    ))
}
fn failure(code: ProtocolErrorCode, message: impl Into<String>) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code,
        message: message.into(),
    })
}
fn map_tool_error(error: ToolError) -> WipOperationError {
    match error {
        ToolError::InvalidArgument(message) => {
            failure(ProtocolErrorCode::InvalidArguments, message)
        }
        ToolError::Cancelled(output) => WipOperationError::Cancelled(output),
        ToolError::Interrupted(output) => WipOperationError::Interrupted(output),
        ToolError::StructuredConflict { code, .. } => {
            let (code, message) = match code.as_str() {
                "workspace_http_401" | "workspace_http_403" => (
                    ProtocolErrorCode::PermissionDenied,
                    "Workspace authority denied the lifecycle request",
                ),
                "workspace_http_404" => (ProtocolErrorCode::NotFound, "Object no longer exists"),
                "workspace_http_409" => (
                    ProtocolErrorCode::ValidatorMismatch,
                    "connection or lifecycle observation is stale; rediscover",
                ),
                _ => {
                    return WipOperationError::OutcomeUnknown(
                        "Workspace lifecycle conflict with unknown execution outcome".into(),
                    );
                }
            };
            failure(code, message)
        }
        // Never expose raw backend errors (which may include host/provider details).
        _ => WipOperationError::OutcomeUnknown(
            "Workspace lifecycle operation failed; outcome unknown; do not automatically retry"
                .into(),
        ),
    }
}
fn output_json(output: ToolOutput) -> Result<Json, WipOperationError> {
    serde_json::from_str(output.content.as_deref().unwrap_or_default())
        .map_err(|_| WipOperationError::OutcomeUnknown("invalid lifecycle result".into()))
}
fn argument_json(arguments: &BTreeMap<String, Value>) -> Result<Json, WipOperationError> {
    let mut values = serde_json::Map::new();
    for (name, value) in arguments {
        if !matches!(value, Value::Unit) {
            values.insert(
                name.clone(),
                wip_to_json(value).map_err(|e| failure(ProtocolErrorCode::InvalidArguments, e))?,
            );
        }
    }
    Ok(Json::Object(values))
}
fn page_limit(input: &Json) -> Result<usize, WipOperationError> {
    let limit = match input.get("limit") {
        None | Some(Json::Null) => 50,
        Some(value) => value.as_u64().ok_or_else(|| {
            failure(
                ProtocolErrorCode::InvalidArguments,
                "limit must be an integer in 1..=100",
            )
        })?,
    };
    if !(1..=100).contains(&limit) {
        return Err(failure(
            ProtocolErrorCode::InvalidArguments,
            "limit must be 1..=100",
        ));
    }
    Ok(limit as usize)
}

// Readable ASCII identities remain unchanged. Every other UTF-8 identity uses a
// canonical ~hex segment; the escape marker is itself always escaped. This is
// injective, does not exclude existing keys, and never decodes a second route.
pub(crate) fn encode_identity(id: &str) -> String {
    if !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        id.into()
    } else {
        format!(
            "~{}",
            id.as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    }
}
pub(crate) fn decode_identity(segment: &str) -> Option<String> {
    let id = if let Some(hex) = segment.strip_prefix('~') {
        if hex.len() % 2 != 0 || hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        String::from_utf8(bytes).ok()?
    } else {
        segment.into()
    };
    (encode_identity(&id) == segment && !id.is_empty()).then_some(id)
}
fn item_path(domain: Domain, id: &str) -> String {
    format!("{}/{}", domain.route(), encode_identity(id))
}
fn parameter(name: &str, required: bool, r#type: TypeExpr) -> ParameterDeclaration {
    ParameterDeclaration {
        name: name.into(),
        required,
        documentation: None,
        r#type,
    }
}
fn operation(
    name: &str,
    summary: &str,
    parameters: Vec<ParameterDeclaration>,
) -> OperationDeclaration {
    OperationDeclaration {
        name: name.into(),
        documentation: Some(Documentation {
            summary: summary.into(),
            details: None,
        }),
        parameters,
        returns: ReturnDeclaration {
            documentation: None,
            r#type: TypeExpr::Json,
        },
    }
}
fn descriptor(
    domain: Domain,
    item: bool,
    operations: Vec<OperationDeclaration>,
) -> InterfaceDescriptor {
    InterfaceDescriptor { format: INTERFACE_FORMAT_V1.into(), documentation: Some(Documentation {
        summary: format!("Workspace {} {}", domain.owner(), if item { "item" } else { "collection" }),
        details: Some("Object identity is route-bound. Feature implementation, target capability and subject permission are independent. Inventory does not grant content or command access.".into()),
    }), types: Vec::new(), operations }
}
fn collection_descriptor(domain: Domain, management: bool) -> InterfaceDescriptor {
    descriptor(
        domain,
        false,
        if management {
            vec![operation(
                "create",
                CREATE_DESCRIPTION,
                vec![
                    parameter("repository_key", true, TypeExpr::String),
                    parameter("selector", false, TypeExpr::String),
                    parameter("runtime_id", false, TypeExpr::String),
                    parameter("display_name", false, TypeExpr::String),
                ],
            )]
        } else {
            vec![operation(
                "list",
                "List a bounded page of visible Objects with item paths",
                vec![
                    parameter("limit", false, TypeExpr::Integer),
                    parameter("cursor", false, TypeExpr::String),
                ],
            )]
        },
    )
}
fn item_descriptor(domain: Domain, management: bool) -> InterfaceDescriptor {
    descriptor(
        domain,
        true,
        if management {
            match domain {
                Domain::Workdir => vec![
                    operation(
                        "attach",
                        ATTACH_DESCRIPTION,
                        vec![parameter("alias", true, TypeExpr::String)],
                    ),
                    operation(
                        "delete",
                        DELETE_DESCRIPTION,
                        vec![parameter("reason", true, TypeExpr::String)],
                    ),
                ],
                Domain::Attachment => vec![operation("detach", DETACH_DESCRIPTION, vec![])],
                Domain::Repository => vec![],
            }
        } else {
            vec![operation(
                "read",
                "Read this Object through current Workspace authority",
                vec![],
            )]
        },
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use manifest::ToolPermissionRule;
    use std::sync::Mutex;

    #[derive(Debug)]
    pub(crate) struct CatalogClient {
        pub(crate) state: Mutex<CatalogState>,
    }
    #[derive(Debug)]
    pub(crate) struct CatalogState {
        pub(crate) repositories: Vec<Json>,
        pub(crate) workdirs: Vec<Json>,
        pub(crate) attachments: Vec<Json>,
        pub(crate) requests: Vec<WorkspaceRequest>,
        pub(crate) denied: bool,
        config_current: Option<Json>,
        generation: usize,
    }
    impl Default for CatalogClient {
        fn default() -> Self {
            Self {
                state: Mutex::new(CatalogState {
                    repositories: vec![
                        json!({"repository_key":"registered-key", "kind":"git", "provider":"git", "source":{"path":"/secret/host", "credential":"SECRET"}, "git":{"remotes":["secret-uri"]}}),
                    ],
                    workdirs: vec![],
                    attachments: vec![],
                    requests: vec![],
                    denied: false,
                    config_current: None,
                    generation: 0,
                }),
            }
        }
    }
    fn directory(id: &str, repository: &str) -> Json {
        json!({"working_directory_id":id, "source":{"kind":"repository", "repository_key":repository}, "display_name":"Review checkout", "status":"active", "cleanliness":"clean", "materializer_kind":"runtime_git_clone", "cleanup_target":{"kind":"runtime_git_clone", "repository_key":repository, "working_directory_id":id}})
    }
    impl WorkspaceClient for CatalogClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("test-workspace")
        }
        fn kind(&self) -> &str {
            "catalog-test"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            let mut state = self.state.lock().unwrap();
            state.requests.push(request.clone());
            if state.denied {
                return Ok(WorkspaceResponse {
                    status: 403,
                    body: "SECRET /host/path".into(),
                });
            }
            let path = request.path.strip_prefix("/api/w/test-workspace/").unwrap();
            let body: Json = request
                .body
                .as_deref()
                .map(|b| serde_json::from_str(b).unwrap())
                .unwrap_or(Json::Null);
            let value = match (request.method, path.split('?').next().unwrap()) {
                (WorkspaceRequestMethod::Get, "workers/self/workspace-config") => {
                    state.config_current.clone().unwrap_or(Json::Null)
                }
                (WorkspaceRequestMethod::Get, "repositories") => {
                    json!({"items":state.repositories})
                }
                (WorkspaceRequestMethod::Get, "working-directories") => {
                    // The legacy browser/Tool endpoint is a latest-200 snapshot,
                    // not a complete WIP catalog; regressions must not use it.
                    json!({"items":state.workdirs.iter().rev().take(200).collect::<Vec<_>>()})
                }
                (WorkspaceRequestMethod::Get, "workers/self/workdir-catalog") => {
                    let query: BTreeMap<_, _> = path
                        .split('?')
                        .nth(1)
                        .unwrap_or_default()
                        .split('&')
                        .filter_map(|p| p.split_once('='))
                        .collect();
                    let limit = query
                        .get("limit")
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(50);
                    let cursor = query.get("cursor").copied();
                    let mut all: Vec<_> = state.workdirs.iter().collect();
                    all.sort_by_key(|v| v["working_directory_id"].as_str().unwrap());
                    let revision = Sha256::digest(serde_json::to_vec(&all).unwrap())
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>();
                    let remaining: Vec<_> = all
                        .into_iter()
                        .filter(|v| {
                            cursor.is_none_or(|c| v["working_directory_id"].as_str().unwrap() > c)
                        })
                        .collect();
                    let has_more = remaining.len() > limit;
                    let items: Vec<_> = remaining.into_iter().take(limit).collect();
                    let next = if has_more {
                        items.last().map(|v| v["working_directory_id"].clone())
                    } else {
                        None
                    };
                    json!({"items":items, "next_cursor":next, "revision":revision})
                }
                (WorkspaceRequestMethod::Get, "workers/self/workdir-attachments") => {
                    let query: BTreeMap<_, _> = path
                        .split('?')
                        .nth(1)
                        .unwrap_or_default()
                        .split('&')
                        .filter_map(|p| p.split_once('='))
                        .collect();
                    let mut all: Vec<_> = state.attachments.iter().collect();
                    all.sort_by_key(|v| v["alias"].as_str().unwrap());
                    let revision = Sha256::digest(serde_json::to_vec(&all).unwrap())
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>();
                    if let Some(expected) = query.get("connection_id") {
                        json!({"items":all.into_iter().filter(|a| a["connection_id"] == *expected).collect::<Vec<_>>(), "next_offset":null, "revision":revision})
                    } else {
                        let limit = query
                            .get("limit")
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(50);
                        let offset = query
                            .get("offset")
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(0);
                        let count = all.len();
                        let items: Vec<_> = all.into_iter().skip(offset).take(limit).collect();
                        let next = (offset + limit < count).then_some(offset + limit);
                        json!({"items":items, "next_offset":next, "revision":revision})
                    }
                }
                (WorkspaceRequestMethod::Get, p) if p.starts_with("repositories/") => {
                    let id = p.trim_start_matches("repositories/");
                    match state
                        .repositories
                        .iter()
                        .find(|r| encode_path_segment(r["repository_key"].as_str().unwrap()) == id)
                    {
                        Some(r) => json!({"item":r}),
                        None => {
                            return Ok(WorkspaceResponse {
                                status: 404,
                                body: String::new(),
                            });
                        }
                    }
                }
                (WorkspaceRequestMethod::Get, p) if p.starts_with("working-directories/") => {
                    let id = p.trim_start_matches("working-directories/");
                    match state
                        .workdirs
                        .iter()
                        .find(|r| r["working_directory_id"] == id)
                    {
                        Some(r) => json!({"item":r}),
                        None => {
                            return Ok(WorkspaceResponse {
                                status: 404,
                                body: String::new(),
                            });
                        }
                    }
                }
                (WorkspaceRequestMethod::Post, "working-directories") => {
                    let raw = directory("wd-created", body["repository_key"].as_str().unwrap());
                    state.workdirs.push(raw.clone());
                    json!({"workspace_id":"test-workspace", "runtime_id":"runtime", "item":raw, "diagnostics":[]})
                }
                (WorkspaceRequestMethod::Post, "workers/self/workdir-attachments") => {
                    state.generation += 1;
                    let raw = json!({"alias":body["alias"], "working_directory_id":body["working_directory_id"], "connection_id":format!("connection-{}", state.generation), "capabilities":{"bits":63}});
                    state.attachments.push(raw);
                    json!({"workspace_id":"test-workspace", "alias":body["alias"], "working_directory_id":body["working_directory_id"], "capabilities":{"bits":63}, "attached":true})
                }
                (WorkspaceRequestMethod::Delete, p)
                    if p.starts_with("workers/self/workdir-attachments/") =>
                {
                    let alias = p.trim_start_matches("workers/self/workdir-attachments/");
                    let expected = path
                        .split("expected_connection_id=")
                        .nth(1)
                        .unwrap_or_default();
                    let Some(index) = state
                        .attachments
                        .iter()
                        .position(|r| r["alias"] == alias && r["connection_id"] == expected)
                    else {
                        return Ok(WorkspaceResponse {
                            status: 409,
                            body: "stale".into(),
                        });
                    };
                    let raw = state.attachments.remove(index);
                    json!({"workspace_id":"test-workspace", "alias":raw["alias"], "working_directory_id":raw["working_directory_id"], "capabilities":raw["capabilities"], "attached":false})
                }
                (WorkspaceRequestMethod::Delete, p) if p.starts_with("working-directories/") => {
                    json!({"working_directory_id":p.trim_start_matches("working-directories/"), "disposition":"retained", "retryable":false})
                }
                other => panic!("unexpected catalog request: {other:?}"),
            };
            Ok(WorkspaceResponse {
                status: 200,
                body: value.to_string(),
            })
        }
    }
    #[test]
    fn config_content_reference_requires_enabled_mount_and_exact_current_lifetime() {
        let client = Arc::new(CatalogClient::default());
        let current = json!({"workspace_id":"test-workspace", "alias":"workspace-config", "connection_id":"config-lifetime", "working_directory_id":"config-workdir", "access":"read_only", "name":"Workspace configuration", "purpose":"Configuration editing", "content_path":"/workspace-config", "already_attached":true});
        client.state.lock().unwrap().config_current = Some(current);
        let raw = json!({"alias":"workspace-config", "connection_id":"config-lifetime", "working_directory_id":"config-workdir", "capabilities":{"bits":25}, "host_path":"/SECRET"});
        let mut p = provider(client.clone(), true, true, None);
        assert!(
            p.projected(Domain::Attachment, &raw)
                .ok()
                .unwrap()
                .get("content_path")
                .is_none()
        );
        p.config_content = true;
        let mut physical = raw.clone();
        physical["alias"] = json!("checkout");
        assert!(
            p.projected(Domain::Attachment, &physical)
                .ok()
                .unwrap()
                .get("content_path")
                .is_none()
        );
        assert!(
            client.state.lock().unwrap().requests.is_empty(),
            "physical inventories must not perform a config request per attachment"
        );
        let projected = p.projected(Domain::Attachment, &raw).ok().unwrap();
        assert_eq!(projected["content_path"], "/workspace-config");
        assert_eq!(projected["access"], "read_only");
        assert_eq!(projected["purpose"], "Configuration editing");
        assert!(projected.get("checkout_path").is_none());
        assert!(!projected.to_string().contains("SECRET"));
        let mut old = raw.clone();
        old["connection_id"] = json!("previous-lifetime");
        assert!(
            p.projected(Domain::Attachment, &old)
                .ok()
                .unwrap()
                .get("content_path")
                .is_none()
        );
        client.state.lock().unwrap().denied = true;
        assert!(
            p.projected(Domain::Attachment, &raw)
                .ok()
                .unwrap()
                .get("content_path")
                .is_none()
        );
    }

    #[test]
    fn config_workdir_source_preserves_safe_mapping_not_private_management_data() {
        let p = provider(Arc::new(CatalogClient::default()), true, false, None);
        let raw = json!({"working_directory_id":"config-workdir", "source":{"kind":"workspace_config", "access":"read_write", "content_path":"/workspace-config", "purpose":"Configuration editing", "grant_id":"PRIVATE", "host_path":"/SECRET"}});
        let projected = p.projected(Domain::Workdir, &raw).ok().unwrap();
        assert_eq!(
            projected["source"],
            json!({"kind":"workspace_config", "access":"read_write", "content_path":"/workspace-config", "purpose":"Configuration editing"})
        );
        assert!(!projected.to_string().contains("PRIVATE"));
        assert!(!projected.to_string().contains("SECRET"));
    }

    fn provider(
        client: Arc<CatalogClient>,
        catalog: bool,
        manage: bool,
        permissions: Option<ToolPermissionConfig>,
    ) -> Provider {
        Provider {
            backend: ManageWorkdirFeature::new(client).backend(),
            catalog,
            manage,
            config_content: false,
            permissions,
        }
    }
    fn arguments(input: Json) -> BTreeMap<String, Value> {
        input
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), json_to_wip(v).unwrap()))
            .collect()
    }
    fn context() -> WipCallContext {
        WipCallContext {
            execution: ToolExecutionContext::default(),
            security_context: "test".into(),
        }
    }
    async fn call(handler: &dyn WipOperationHandler, operation: &str, input: Json) -> Json {
        let output = handler
            .call(operation, &arguments(input), context())
            .await
            .unwrap_or_else(|_| panic!("{operation} failed"));
        wip_to_json(&output.value).unwrap()
    }
    fn policy(allowed: &[&str]) -> ToolPermissionConfig {
        ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: allowed
                .iter()
                .map(|name| ToolPermissionRule {
                    tool: (*name).into(),
                    pattern: "*".into(),
                    action: ToolPermissionAction::Allow,
                })
                .collect(),
        }
    }

    #[test]
    fn catalog_route_encoding_is_injective_and_canonical_for_existing_keys() {
        let keys = [
            "main",
            "Main",
            "repo/name",
            "../main",
            ".",
            "日本語",
            "~2e",
            "a.b",
            "%2f",
            "with space",
        ];
        let mut paths = std::collections::HashSet::new();
        for key in keys {
            let encoded = encode_identity(key);
            assert_eq!(decode_identity(&encoded).as_deref(), Some(key));
            assert!(paths.insert(encoded.clone()));
            wip_protocol::validate_path(&format!("/repositories/{encoded}")).unwrap();
        }
        for invalid in ["~", "~xx", "~é", "~6162", "a.b", "..", "~C3A9"] {
            assert!(decode_identity(invalid).is_none(), "{invalid}");
        }
    }

    #[tokio::test]
    async fn catalog_fresh_workspace_find_key_create_attach_read_detach_preserves_workdir() {
        let client = Arc::new(CatalogClient::default());
        let p = provider(client.clone(), true, true, None);
        let repositories = CollectionHandler {
            provider: p.clone(),
            domain: Domain::Repository,
        };
        let listed = call(&repositories, "list", json!({})).await;
        assert_eq!(listed["items"][0]["repository_key"], "registered-key");
        assert_eq!(listed["items"][0]["path"], "/repositories/registered-key");
        assert!(listed["items"][0]["default_selector"].is_null());
        assert!(!listed.to_string().contains("SECRET"));
        assert!(!listed.to_string().contains("/secret/host"));
        let repository = ItemHandler {
            provider: p.clone(),
            domain: Domain::Repository,
            id: "registered-key".into(),
        };
        assert!(!repository.operation_available("create"));
        let read = call(&repository, "read", json!({})).await;
        let workdirs = CollectionHandler {
            provider: p.clone(),
            domain: Domain::Workdir,
        };
        let created = call(
            &workdirs,
            "create",
            json!({"repository_key":read["repository_key"]}),
        )
        .await;
        assert_eq!(created["item"]["path"], "/workdirs/wd-created");
        assert!(
            client.state.lock().unwrap().attachments.is_empty(),
            "create is not attach"
        );
        let workdir = ItemHandler {
            provider: p.clone(),
            domain: Domain::Workdir,
            id: "wd-created".into(),
        };
        let attached = call(&workdir, "attach", json!({"alias":"checkout"})).await;
        assert_eq!(attached["attachments_path"], ATTACHMENTS);
        let attachments = CollectionHandler {
            provider: p.clone(),
            domain: Domain::Attachment,
        };
        let listed = call(&attachments, "list", json!({})).await;
        assert_eq!(listed["items"][0]["alias"], "checkout");
        assert_eq!(listed["items"][0]["workdir_path"], "/workdirs/wd-created");
        let connection = ItemHandler {
            provider: p.clone(),
            domain: Domain::Attachment,
            id: listed["items"][0]["connection_id"].as_str().unwrap().into(),
        };
        call(&connection, "detach", json!({})).await;
        assert!(client.state.lock().unwrap().attachments.is_empty());
        assert_eq!(
            client.state.lock().unwrap().workdirs.len(),
            1,
            "detach is not delete"
        );
        assert!(p.backend.session_router.aliases().is_empty());
        let retained = call(&workdir, "delete", json!({"reason":"done"})).await;
        assert_eq!(retained["disposition"], "retained");
    }

    #[tokio::test]
    async fn catalog_feature_permission_and_target_capability_are_independent() {
        let client = Arc::new(CatalogClient::default());
        client
            .state
            .lock()
            .unwrap()
            .workdirs
            .push(directory("wd", "registered-key"));
        for manage in [false, true] {
            let p = provider(
                client.clone(),
                true,
                manage,
                Some(policy(&[
                    "RepositoryList",
                    "RepositoryRead",
                    LIST_TOOL,
                    "WorkdirRead",
                ])),
            );
            let handler = ItemHandler {
                provider: p.clone(),
                domain: Domain::Workdir,
                id: "wd".into(),
            };
            assert!(handler.operation_available("read"));
            assert!(!handler.operation_available("attach"));
            assert!(matches!(
                handler
                    .call("attach", &arguments(json!({"alias":"x"})), context())
                    .await,
                Err(WipOperationError::Protocol(_))
            ));
            let resolver = ItemResolver {
                provider: p,
                domain: Domain::Workdir,
            };
            assert!(resolver.resolve("wd").is_some());
        }
        let denied = provider(client.clone(), true, true, Some(policy(&[])));
        assert!(
            ItemResolver {
                provider: denied.clone(),
                domain: Domain::Workdir
            }
            .resolve("wd")
            .is_none()
        );
        assert!(
            !CollectionHandler {
                provider: denied,
                domain: Domain::Repository
            }
            .is_visible()
        );
        let p = provider(client.clone(), true, true, None);
        client.state.lock().unwrap().workdirs.push(json!({"working_directory_id":"external", "status":"active", "source":{"kind":"external_grant", "grant_id":"opaque", "grant_permissions":{"read":true}}}));
        let external = ItemHandler {
            provider: p.clone(),
            domain: Domain::Workdir,
            id: "external".into(),
        };
        assert!(external.operation_available("attach"));
        assert!(!external.operation_available("delete"));
        let raw = call(&external, "read", json!({})).await;
        assert!(raw["source"].get("grant_id").is_none());
        client.state.lock().unwrap().denied = true;
        assert!(!external.operation_available("read"));
        assert!(
            ItemResolver {
                provider: p,
                domain: Domain::Workdir
            }
            .resolve("external")
            .is_none()
        );
        assert!(
            client
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|r| r.method == WorkspaceRequestMethod::Get)
        );
    }

    #[tokio::test]
    async fn catalog_empty_paging_route_bound_identity_and_failure_classification() {
        let client = Arc::new(CatalogClient::default());
        let p = provider(client.clone(), true, true, None);
        let handler = CollectionHandler {
            provider: p.clone(),
            domain: Domain::Repository,
        };
        client.state.lock().unwrap().repositories.clear();
        let empty = call(&handler, "list", json!({})).await;
        assert_eq!(empty["empty"], true);
        client.state.lock().unwrap().repositories = (0..3)
            .map(|i| json!({"repository_key":format!("key-{i}")}))
            .collect();
        let first = call(&handler, "list", json!({"limit":2})).await;
        assert_eq!(first["items"].as_array().unwrap().len(), 2);
        let second = call(
            &handler,
            "list",
            json!({"limit":2, "cursor":first["next_cursor"]}),
        )
        .await;
        assert_eq!(second["items"][0]["repository_key"], "key-2");
        assert_eq!(second["has_more"], false);
        assert!(
            handler
                .call("list", &arguments(json!({"limit":101})), context())
                .await
                .is_err()
        );
        let item = ItemHandler {
            provider: p,
            domain: Domain::Workdir,
            id: "wd-bound".into(),
        };
        let requests_before = client.state.lock().unwrap().requests.len();
        assert!(
            item.call(
                "delete",
                &arguments(json!({"working_directory_id":"different", "reason":"done"})),
                context()
            )
            .await
            .is_err()
        );
        assert_eq!(client.state.lock().unwrap().requests.len(), requests_before);
        assert!(matches!(
            map_tool_error(ToolError::StructuredConflict {
                code: "workspace_http_409".into(),
                message: "SECRET".into()
            }),
            WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::ValidatorMismatch,
                ..
            })
        ));
        assert!(matches!(
            map_tool_error(ToolError::ExecutionFailed(
                "transport uncertain SECRET".into()
            )),
            WipOperationError::OutcomeUnknown(_)
        ));
    }

    #[test]
    fn catalog_registration_uses_host_namespace_and_rejects_conflicts() {
        let feature = ManageWorkdirFeature::new(Arc::new(CatalogClient::default()));
        let mut registry = WipMountRegistry::new();
        mount_workspace_workdir_wip(&mut registry, &feature, true, true, None).unwrap();
        assert_eq!(
            registry.routes().collect::<Vec<_>>(),
            [REPOSITORIES, ATTACHMENTS, WORKDIRS]
        );
        assert!(matches!(
            mount_workspace_workdir_wip(&mut registry, &feature, true, true, None),
            Err(WipMountError::RouteCollision { .. })
        ));
        assert!(matches!(
            registry.contribute_operations(WipOperationContribution {
                route: WORKDIRS.into(),
                contributor: "another-feature".into(),
                interface: "yoi.workdir/collection/v1".into(),
                descriptor: collection_descriptor(Domain::Workdir, true),
                handler: Arc::new(CollectionHandler {
                    provider: provider(Arc::new(CatalogClient::default()), true, true, None),
                    domain: Domain::Workdir
                })
            }),
            Err(WipMountError::OperationCollision { .. })
        ));
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::tests::CatalogClient;
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn attachment_pages_and_direct_lookup_are_not_limited_to_first_page() {
        let client = Arc::new(CatalogClient::default());
        client.state.lock().unwrap().attachments = (0..65).map(|n| json!({"connection_id":format!("lifetime-{n:03}"), "alias":format!("alias-{n:03}"), "working_directory_id":"wd", "capabilities":{"bits":25}})).collect();
        let p = Provider {
            backend: ManageWorkdirFeature::new(client).backend(),
            permissions: None,
            catalog: true,
            manage: true,
            config_content: false,
        };
        let handler = CollectionHandler {
            provider: p.clone(),
            domain: Domain::Attachment,
        };
        let args = BTreeMap::from([("limit".into(), Value::Integer(50))]);
        let context = || WipCallContext {
            execution: ToolExecutionContext::default(),
            security_context: "test".into(),
        };
        let first = handler
            .call("list", &args, context())
            .await
            .unwrap_or_else(|_| panic!("list failed"));
        let first = wip_to_json(&first.value).unwrap();
        assert_eq!(first["items"].as_array().unwrap().len(), 50);
        assert_eq!(first["next_cursor"], "offset:50");
        let args = BTreeMap::from([("cursor".into(), Value::String("offset:50".into()))]);
        let second = handler
            .call("list", &args, context())
            .await
            .unwrap_or_else(|_| panic!("list failed"));
        let second = wip_to_json(&second.value).unwrap();
        assert_eq!(second["items"].as_array().unwrap().len(), 15);
        assert_eq!(second["has_more"], false);
        let resolver = ItemResolver {
            provider: p,
            domain: Domain::Attachment,
        };
        assert!(resolver.resolve("lifetime-064").is_some());
        assert!(resolver.resolve("lifetime-unknown").is_none());
    }

    #[tokio::test]
    async fn stale_lifetime_preflight_never_closes_new_alias_or_stops_children() {
        let client = Arc::new(CatalogClient::default());
        client.state.lock().unwrap().attachments.push(json!({"connection_id":"replacement", "alias":"checkout", "working_directory_id":"same-workdir", "capabilities":{"bits":63}}));
        let hooks = Arc::new(AtomicUsize::new(0));
        let hook_calls = hooks.clone();
        let backend = WorkspaceHttpWorkdirBackend::new(client.clone()).with_child_lifecycle(
            Some(Arc::new(move || {
                let hook_calls = hook_calls.clone();
                Box::pin(async move {
                    hook_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })),
            None,
        );
        let alias = workdir::WorkdirAttachmentAlias::new("checkout").unwrap();
        backend
            .session_router
            .attach(
                alias.clone(),
                WorkspaceAttachedWorkdirSession::handle_for_workdir(
                    client.clone(),
                    "checkout",
                    "same-workdir",
                ),
            )
            .unwrap();
        let error = backend
            .detach_managed("checkout", Some("old-lifetime"))
            .await
            .unwrap_err();
        assert!(matches!(error, ToolError::StructuredConflict { .. }));
        assert_eq!(hooks.load(Ordering::SeqCst), 0);
        assert!(backend.session_router.session(&alias).is_some());
        assert_eq!(
            client.state.lock().unwrap().attachments[0]["connection_id"],
            "replacement"
        );
        assert!(
            client
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|r| r.method == WorkspaceRequestMethod::Get)
        );
    }
}

#[cfg(test)]
mod logical_capability_tests {
    use super::*;
    #[test]
    fn logical_reference_does_not_gain_physical_attach_delete_or_command() {
        let client = Arc::new(super::tests::CatalogClient::default());
        client.state.lock().unwrap().workdirs.push(json!({"working_directory_id":"logical-settings", "source":{"kind":"workspace_config"}, "status":"active"}));
        let provider = Provider {
            backend: ManageWorkdirFeature::new(client).backend(),
            catalog: true,
            manage: true,
            config_content: false,
            permissions: None,
        };
        let item = ItemHandler {
            provider,
            domain: Domain::Workdir,
            id: "logical-settings".into(),
        };
        assert!(item.operation_available("read"));
        for operation in ["attach", "delete", "command"] {
            assert!(!item.operation_available(operation));
        }
    }
}

#[cfg(test)]
mod page_contract_tests {
    use super::*;
    #[test]
    fn bounded_page_limit_rejects_negative_and_noninteger_inputs() {
        assert_eq!(page_limit(&json!({})).unwrap_or_default(), 50);
        assert_eq!(page_limit(&json!({"limit":100})).unwrap_or_default(), 100);
        for input in [
            json!({"limit":-1}),
            json!({"limit":0}),
            json!({"limit":101}),
            json!({"limit":"50"}),
            json!({"limit":1.5}),
        ] {
            assert!(matches!(
                page_limit(&input),
                Err(WipOperationError::Protocol(ProtocolError {
                    code: ProtocolErrorCode::InvalidArguments,
                    ..
                }))
            ));
        }
    }
}
