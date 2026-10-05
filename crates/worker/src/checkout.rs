//! Provider-owned request-time checkout projection. No filesystem path is opened
//! by the Worker: every observation and operation goes through the live router.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use agen::tool::ToolError;
use async_trait::async_trait;
use manifest::{ToolPermissionAction, ToolPermissionConfig};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use wip_protocol::{
    Documentation, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, TypeExpr, Value,
};
use workdir::{
    CheckoutObservation, EntryKind, ListRequest, WorkdirPath, WorkdirSessionCapability as Cap,
    WorkdirSessionRouter,
};

use crate::feature::builtin::manage_workdir::wip::{decode_identity, encode_identity};
use crate::permission::permission_action_for;
use crate::wip::{
    WipCallContext, WipMountError, WipMountRegistry, WipOperationError, WipOperationHandler,
    WipOperationOutput, WipProjection, WipProjectionKind, WipSubtreeMount, WipSubtreeProvider,
    json_to_wip, wip_to_json,
};

const ROOT: &str = "/checkouts";
const MAX_NODES: usize = 1024;
const DEADLINE: Duration = Duration::from_secs(30);

/// Return a stable, injective attachment-local entrance, never a host path.
pub fn checkout_root(alias: &str) -> String {
    format!("{ROOT}/{}", encode_identity(alias))
}

/// Register the native content namespace independently of inventory/lifecycle
/// Features. Its capabilities come from current attachments, not Feature flags.
pub fn mount_checkouts(
    registry: &mut WipMountRegistry,
    router: Arc<WorkdirSessionRouter>,
    tracker: tools::Tracker,
    permissions: Option<ToolPermissionConfig>,
) -> Result<(), WipMountError> {
    registry.allocate_namespace("checkout", "checkouts")?;
    let provider = Arc::new(Provider {
        router,
        tracker,
        permissions,
        incarnation: uuid::Uuid::now_v7().to_string(),
        permits: Arc::new(tokio::sync::Semaphore::new(16)),
        deadline: DEADLINE,
    });
    registry.mount(provider.collection())?;
    registry.replace_compatibility_tools(ROOT, ["Read", "Edit", "Write", "Glob", "Grep"])?;
    registry.mount_subtree(WipSubtreeMount {
        root: ROOT.into(),
        provider: Arc::new(provider),
    })
}

struct Provider {
    router: Arc<WorkdirSessionRouter>,
    tracker: tools::Tracker,
    permissions: Option<ToolPermissionConfig>,
    incarnation: String,
    permits: Arc<tokio::sync::Semaphore>,
    deadline: Duration,
}

impl Provider {
    fn allowed(&self, tool: &str, input: &Json) -> bool {
        self.permissions
            .as_ref()
            .is_none_or(|p| permission_action_for(p, tool, input) == ToolPermissionAction::Allow)
    }
    fn may_allow(&self, tool: &str) -> bool {
        let Some(p) = &self.permissions else {
            return true;
        };
        for rule in p.rules.iter().filter(|r| r.tool.eq_ignore_ascii_case(tool)) {
            if rule.action == ToolPermissionAction::Allow {
                return true;
            }
            if rule.pattern == "*" {
                return false;
            }
        }
        p.default_action == ToolPermissionAction::Allow
    }
    fn parse(&self, path: &str) -> Option<(String, WorkdirPath)> {
        wip_protocol::validate_path(path).ok()?;
        let rest = path.strip_prefix("/checkouts/")?;
        let (slug, tail) = rest.split_once('/').unwrap_or((rest, ""));
        if tail.contains('\0') {
            return None;
        }
        Some((decode_identity(slug)?, WorkdirPath::new(tail).ok()?))
    }
    fn wrap_validator(&self, alias: &str, generation: u64, workdir: &str, raw: &[u8]) -> Vec<u8> {
        let mut hash = Sha256::new();
        for value in [
            self.incarnation.as_bytes(),
            alias.as_bytes(),
            &generation.to_be_bytes(),
            workdir.as_bytes(),
            raw,
        ] {
            hash.update(value.len().to_be_bytes());
            hash.update(value);
        }
        hash.finalize().to_vec()
    }
    fn collection(self: &Arc<Self>) -> WipProjection {
        let mut state = Sha256::new();
        state.update(&self.incarnation);
        for alias in self.router.aliases() {
            if let Ok(s) = self.router.resolve(Some(alias.as_str())) {
                state.update(alias.as_str());
                state.update(s.generation.to_be_bytes());
                state.update(s.session.workdir().id().as_str());
                state.update(format!("{:?}", s.session.capabilities()));
            }
        }
        self.project(
            ROOT,
            descriptor(vec![operation(
                "list",
                "List currently accessible checkouts",
                vec![],
            )]),
            state.finalize().to_vec(),
            "collection",
            Arc::new(Collection {
                provider: self.clone(),
            }),
        )
    }
    fn project(
        &self,
        path: &str,
        descriptor: InterfaceDescriptor,
        validator: Vec<u8>,
        kind: &str,
        handler: Arc<dyn WipOperationHandler>,
    ) -> WipProjection {
        let encoded: String = path.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let interface = format!("yoi.checkout/{}/{kind}/v1/@/{encoded}", self.incarnation);
        let interface_validator = Sha256::digest(format!("{descriptor:?}")).to_vec();
        WipProjection {
            route: path.into(),
            capability: format!("checkout:{kind}"),
            kind: WipProjectionKind::Native,
            object: Object {
                name: path.rsplit('/').next().unwrap_or_default().into(),
                description: Some(format!("Attached Workdir {kind}")),
                interfaces: vec![interface.clone()],
                r#ref: None,
                validator: Some(validator),
            },
            interface,
            descriptor,
            interface_validator: Some(interface_validator),
            handler,
        }
    }
    async fn node(self: &Arc<Self>, path: &str) -> Result<Option<WipProjection>, ProtocolError> {
        if path == ROOT {
            return Ok(Some(self.collection()));
        }
        let Some((alias, target)) = self.parse(path) else {
            return Ok(None);
        };
        let Ok(selected) = self.router.resolve(Some(&alias)) else {
            return Ok(None);
        };
        let observation = match selected.session.checkout_observe(target.clone()).await {
            Ok(o) => o,
            Err(
                workdir::WorkdirError::Denied(_)
                | workdir::WorkdirError::OutOfScope(_)
                | workdir::WorkdirError::NotFound(_)
                | workdir::WorkdirError::SessionClosed
                | workdir::WorkdirError::UnsupportedOperation(_)
                | workdir::WorkdirError::Unsupported(_),
            ) => return Ok(None),
            Err(_) => {
                return Err(error(
                    ProtocolErrorCode::Internal,
                    "checkout observation unavailable",
                ));
            }
        };
        if !matches!(observation.kind, EntryKind::File | EntryKind::Directory) {
            return Ok(None);
        }
        let permission_input = json!({"target_workdir":alias, "file_path":target.as_str()});
        if observation.kind == EntryKind::File && !self.allowed("Read", &permission_input) {
            return Ok(None);
        }
        let ops = self.operations(&observation);
        if ops.is_empty() {
            return Ok(None);
        }
        let kind = if observation.kind == EntryKind::File {
            "file"
        } else {
            "directory"
        };
        let workdir = selected.session.workdir().id().as_str().to_owned();
        let validator = self.wrap_validator(
            &alias,
            selected.generation,
            &workdir,
            &observation.validator,
        );
        let handler = Arc::new(FileHandler {
            provider: self.clone(),
            alias,
            generation: selected.generation,
            workdir,
            target,
            observation,
        });
        let family = format!("{kind}-g{}", selected.generation);
        Ok(Some(self.project(
            path,
            descriptor(ops),
            validator,
            &family,
            handler,
        )))
    }
    fn operations(&self, o: &CheckoutObservation) -> Vec<OperationDeclaration> {
        let mut ops = Vec::new();
        let has = |cap, name| o.capabilities.supports(cap) && self.may_allow(name);
        if o.kind == EntryKind::File {
            if has(Cap::Read, "Read") {
                ops.push(operation(
                    "read",
                    "Read text with line offsets and numbered output",
                    vec![
                        parameter("offset", false, TypeExpr::Integer),
                        parameter("limit", false, TypeExpr::Integer),
                    ],
                ));
            }
            if has(Cap::Edit, "Edit") {
                ops.push(operation(
                    "edit",
                    "Replace a unique string (requires prior Read)",
                    vec![
                        parameter("old_string", true, TypeExpr::String),
                        parameter("new_string", true, TypeExpr::String),
                        parameter("replace_all", false, TypeExpr::Boolean),
                    ],
                ));
            }
            if has(Cap::Write, "Write") {
                ops.push(operation(
                    "write",
                    "Overwrite this existing file (requires prior Read)",
                    vec![parameter("content", true, TypeExpr::String)],
                ));
            }
        } else {
            if has(Cap::Glob, "Glob") {
                ops.push(operation(
                    "glob",
                    "Glob within this directory",
                    vec![
                        parameter("pattern", true, TypeExpr::String),
                        parameter("path", false, TypeExpr::String),
                    ],
                ));
            }
            if has(Cap::Grep, "Grep") {
                ops.push(operation(
                    "grep",
                    "Search within this directory with the existing Grep contract",
                    grep_parameters(),
                ));
            }
            if has(Cap::Write, "Write") {
                ops.push(operation("create_file", "Create a new relative file, creating permitted missing parents; never overwrite", vec![parameter("path",true,TypeExpr::String),parameter("content",true,TypeExpr::String)]));
            }
        }
        ops
    }
}

#[async_trait]
impl WipSubtreeProvider for Arc<Provider> {
    async fn projection(&self, path: &str) -> Result<Option<WipProjection>, ProtocolError> {
        let _permit = tokio::time::timeout(self.deadline, self.permits.acquire())
            .await
            .map_err(|_| {
                error(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "checkout concurrency deadline",
                )
            })?
            .map_err(|_| error(ProtocolErrorCode::Internal, "checkout unavailable"))?;
        tokio::time::timeout(self.deadline, self.node(path))
            .await
            .map_err(|_| {
                error(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "checkout observation deadline",
                )
            })?
    }
    async fn children(&self, path: &str) -> Result<Vec<String>, ProtocolError> {
        let _permit = tokio::time::timeout(self.deadline, self.permits.acquire())
            .await
            .map_err(|_| {
                error(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "checkout capacity deadline",
                )
            })?
            .map_err(|_| error(ProtocolErrorCode::Internal, "checkout unavailable"))?;
        tokio::time::timeout(self.deadline, self.children_inner(path))
            .await
            .map_err(|_| {
                error(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "checkout enumeration deadline",
                )
            })?
    }
}

impl Provider {
    async fn children_inner(self: &Arc<Self>, path: &str) -> Result<Vec<String>, ProtocolError> {
        if path == ROOT {
            let before = self.collection().object.validator.unwrap();
            if self.router.aliases().len() > MAX_NODES {
                return Err(error(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "too many checkouts",
                ));
            }
            let mut paths = Vec::new();
            for alias in self.router.aliases() {
                let p = checkout_root(alias.as_str());
                if self.node(&p).await?.is_some() {
                    paths.push(p);
                }
            }
            if before != self.collection().object.validator.unwrap() {
                return Err(error(
                    ProtocolErrorCode::ValidatorMismatch,
                    "attachment collection changed",
                ));
            }
            return Ok(paths);
        }
        let Some((alias, target)) = self.parse(path) else {
            return Err(error(ProtocolErrorCode::NotFound, "checkout not published"));
        };
        let selected = self
            .router
            .resolve(Some(&alias))
            .map_err(|_| error(ProtocolErrorCode::NotFound, "attachment expired"))?;
        let o = selected
            .session
            .checkout_observe(target.clone())
            .await
            .map_err(|_| error(ProtocolErrorCode::NotFound, "checkout not published"))?;
        if o.kind != EntryKind::Directory {
            return Ok(Vec::new());
        }
        if !self.allowed(
            "Glob",
            &json!({"target_workdir":alias,"path":target.as_str(),"pattern":"*"}),
        ) && !self.allowed(
            "Grep",
            &json!({"target_workdir":alias,"path":target.as_str(),"pattern":""}),
        ) {
            return Ok(Vec::new());
        }
        let listing = selected
            .session
            .checkout_search(workdir::CheckoutSearchRequest::new(
                workdir::CheckoutSearchOperation::List(ListRequest {
                    path: target,
                    limit: MAX_NODES + 1,
                }),
            ))
            .await
            .map_err(|_| error(ProtocolErrorCode::Internal, "checkout listing unavailable"))?;
        let workdir::CheckoutSearchResult::List(listing) = listing else {
            return Err(error(
                ProtocolErrorCode::Internal,
                "mismatched checkout listing",
            ));
        };
        if listing.truncated || listing.entries.len() > MAX_NODES {
            return Err(error(
                ProtocolErrorCode::ResourceLimitExceeded,
                "directory observation exceeds node bound",
            ));
        }
        let mut paths = Vec::new();
        for entry in listing.entries {
            let p = object_path(&alias, &entry.path);
            if self.node(&p).await?.is_some() {
                paths.push(p);
            }
        }
        if !self
            .router
            .resolve(Some(&alias))
            .is_ok_and(|current| current.generation == selected.generation)
        {
            return Err(error(
                ProtocolErrorCode::ValidatorMismatch,
                "attachment changed",
            ));
        }
        Ok(paths)
    }
}

struct Collection {
    provider: Arc<Provider>,
}
#[async_trait]
impl WipOperationHandler for Collection {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        _context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        if operation != "list" || !arguments.is_empty() {
            return Err(protocol(
                ProtocolErrorCode::InvalidArguments,
                "list accepts no arguments",
            ));
        }
        let before = self.provider.collection().object.validator.unwrap();
        let paths = Arc::new(self.provider.clone())
            .children(ROOT)
            .await
            .map_err(WipOperationError::Protocol)?;
        let mut items = Vec::new();
        for path in paths {
            let (alias, _) = self.provider.parse(&path).expect("generated checkout path");
            let s = self.provider.router.resolve(Some(&alias)).map_err(|_| {
                protocol(ProtocolErrorCode::ValidatorMismatch, "attachment changed")
            })?;
            items.push(json!({"path":path,"slug":encode_identity(&alias),"alias":alias,
                "working_directory_id":s.session.workdir().id().as_str(),"generation":s.generation}));
        }
        let value = json_to_wip(&json!({"items":items}))
            .map_err(|_| protocol(ProtocolErrorCode::Internal, "invalid checkout result"))?;
        let after = self.provider.collection().object.validator.unwrap();
        if before != after {
            return Err(protocol(
                ProtocolErrorCode::ValidatorMismatch,
                "attachment collection changed",
            ));
        }
        Ok(WipOperationOutput::native_with_validator(value, after))
    }
}

struct FileHandler {
    provider: Arc<Provider>,
    alias: String,
    generation: u64,
    workdir: String,
    target: WorkdirPath,
    observation: CheckoutObservation,
}
impl FileHandler {
    fn check_connection(&self) -> Result<(), WipOperationError> {
        if self
            .provider
            .router
            .resolve(Some(&self.alias))
            .is_ok_and(|s| {
                s.generation == self.generation && s.session.workdir().id().as_str() == self.workdir
            })
        {
            Ok(())
        } else {
            Err(protocol(
                ProtocolErrorCode::ValidatorMismatch,
                "attachment changed while publishing result",
            ))
        }
    }
}
#[async_trait]
impl WipOperationHandler for FileHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let tool = match operation {
            "read" => "Read",
            "edit" => "Edit",
            "write" => "Write",
            "create_file" => "Create",
            "glob" => "Glob",
            "grep" => "Grep",
            _ => {
                return Err(protocol(
                    ProtocolErrorCode::OperationNotFound,
                    "unknown checkout operation",
                ));
            }
        };
        let mut args = serde_json::Map::new();
        for (k, v) in arguments {
            args.insert(
                k.clone(),
                wip_to_json(v).map_err(|_| {
                    protocol(
                        ProtocolErrorCode::InvalidArguments,
                        "invalid checkout argument",
                    )
                })?,
            );
        }
        let mut permission_input = args.clone();
        permission_input.insert("target_workdir".into(), json!(self.alias));
        if tool == "Glob" || tool == "Grep" {
            let relative = args.get("path").and_then(Json::as_str).unwrap_or("");
            permission_input.insert("path".into(), json!(join_under(&self.target, relative)?));
        } else if tool == "Create" {
            let relative = args.get("path").and_then(Json::as_str).ok_or_else(|| {
                protocol(ProtocolErrorCode::InvalidArguments, "create requires path")
            })?;
            permission_input.remove("path");
            permission_input.insert(
                "file_path".into(),
                json!(join_under(&self.target, relative)?),
            );
        } else {
            permission_input.insert("file_path".into(), json!(self.target.as_str()));
        }
        if !self.provider.allowed(
            if tool == "Create" { "Write" } else { tool },
            &Json::Object(permission_input),
        ) {
            return Err(protocol(
                ProtocolErrorCode::PermissionDenied,
                "file operation denied; approval-required policy fails closed",
            ));
        }
        let _permit = tokio::time::timeout(self.provider.deadline, self.provider.permits.acquire())
            .await
            .map_err(|_| {
                protocol(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "checkout operation capacity deadline",
                )
            })?
            .map_err(|_| protocol(ProtocolErrorCode::Internal, "checkout unavailable"))?;
        let deadline = tokio::time::Instant::now() + self.provider.deadline;
        let result = tokio::time::timeout_at(
            deadline,
            tools::execute_checkout_tool(
                self.provider.router.clone(),
                self.provider.tracker.clone(),
                &self.alias,
                self.generation,
                self.target.clone(),
                self.observation.validator.clone(),
                tool,
                Json::Object(args),
                context.execution,
            ),
        )
        .await
        .map_err(|_| {
            if matches!(tool, "Write" | "Edit" | "Create") {
                WipOperationError::OutcomeUnknown("file operation deadline after dispatch".into())
            } else {
                protocol(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "file operation deadline",
                )
            }
        })?
        .map_err(map_tool_error)?;
        let links = tokio::time::timeout_at(deadline, async {
            let mut links = Vec::new();
            self.check_connection()?;
            for path in &result.paths {
                let route = object_path(&self.alias, path);
                // References are validated only against the captured live connection.
                if self
                    .provider
                    .node(&route)
                    .await
                    .map_err(WipOperationError::Protocol)?
                    .is_some()
                {
                    links.push(json!({"path":route}));
                }
                self.check_connection()?;
            }
            Ok::<_, WipOperationError>(links)
        })
        .await
        .map_err(|_| {
            if matches!(tool, "Write" | "Edit" | "Create") {
                WipOperationError::OutcomeUnknown("completed operation response deadline".into())
            } else {
                protocol(
                    ProtocolErrorCode::ResourceLimitExceeded,
                    "checkout result deadline",
                )
            }
        })?
        .map_err(|e| {
            if matches!(tool, "Write" | "Edit" | "Create") {
                WipOperationError::OutcomeUnknown("completed operation response unavailable".into())
            } else {
                e
            }
        })?;
        let value = json_to_wip(
            &json!({"summary":result.output.summary,"content":result.output.content,"items":links}),
        )
        .map_err(|_| {
            WipOperationError::OutcomeUnknown("invalid completed file operation result".into())
        })?;
        if let Some(raw) = result.validator {
            Ok(WipOperationOutput::native_with_validator(
                value,
                self.provider
                    .wrap_validator(&self.alias, self.generation, &self.workdir, &raw),
            ))
        } else {
            Ok(WipOperationOutput::native(value))
        }
    }
}

fn object_path(alias: &str, path: &WorkdirPath) -> String {
    if path.is_root() {
        checkout_root(alias)
    } else {
        format!("{}/{}", checkout_root(alias), path.as_str())
    }
}
fn join_under(parent: &WorkdirPath, relative: &str) -> Result<String, WipOperationError> {
    let child = WorkdirPath::new(relative).map_err(|_| {
        protocol(
            ProtocolErrorCode::InvalidArguments,
            "path must stay relative to this directory",
        )
    })?;
    if relative.starts_with('/') {
        return Err(protocol(
            ProtocolErrorCode::InvalidArguments,
            "absolute paths are not permitted",
        ));
    }
    Ok(if parent.is_root() {
        child.as_str().into()
    } else if child.is_root() {
        parent.as_str().into()
    } else {
        format!("{}/{}", parent.as_str(), child.as_str())
    })
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
fn descriptor(operations: Vec<OperationDeclaration>) -> InterfaceDescriptor {
    InterfaceDescriptor{format:INTERFACE_FORMAT_V1.into(),documentation:Some(Documentation{summary:"AI-oriented attached Workdir operations".into(),details:Some("The route binds attachment and target. Line offsets are zero-based; text output numbers lines from one. Read-before-write and provider validators are both required. Paths are Worldspace references, never host paths.".into())}),types:Vec::new(),operations}
}
fn grep_parameters() -> Vec<ParameterDeclaration> {
    let mut p = vec![parameter("pattern", true, TypeExpr::String)];
    for name in ["path", "glob", "type", "output_mode"] {
        p.push(parameter(name, false, TypeExpr::String));
    }
    for name in ["-A", "-B", "-C", "head_limit", "offset"] {
        p.push(parameter(name, false, TypeExpr::Integer));
    }
    for name in ["case_insensitive", "multiline"] {
        p.push(parameter(name, false, TypeExpr::Boolean));
    }
    p
}
fn error(code: ProtocolErrorCode, message: &str) -> ProtocolError {
    ProtocolError {
        code,
        message: message.into(),
    }
}
fn protocol(code: ProtocolErrorCode, message: &str) -> WipOperationError {
    WipOperationError::Protocol(error(code, message))
}
fn map_tool_error(e: ToolError) -> WipOperationError {
    match e {
        ToolError::InvalidArgument(_) => protocol(
            ProtocolErrorCode::InvalidArguments,
            "invalid file arguments or read-before-write requirement",
        ),
        ToolError::StructuredConflict { code, .. } => match code.as_str() {
            "checkout_denied" => protocol(
                ProtocolErrorCode::PermissionDenied,
                "file operation refused before effects",
            ),
            "checkout_invalid" => protocol(
                ProtocolErrorCode::InvalidArguments,
                "invalid checkout operation or target",
            ),
            "checkout_unavailable" => protocol(
                ProtocolErrorCode::ResourceLimitExceeded,
                "checkout provider declined before effects",
            ),
            "checkout_stale" => protocol(
                ProtocolErrorCode::ValidatorMismatch,
                "file or attachment observation is stale; rediscover and reread",
            ),
            _ => WipOperationError::OutcomeUnknown("unclassified file operation failure".into()),
        },
        ToolError::Cancelled(o) => WipOperationError::Cancelled(o),
        ToolError::Interrupted(o) => WipOperationError::Interrupted(o),
        _ => WipOperationError::OutcomeUnknown(
            "file operation failed; inspect current state before retry".into(),
        ),
    }
}

#[cfg(test)]
#[path = "checkout_race_tests.rs"]
mod race_tests;
