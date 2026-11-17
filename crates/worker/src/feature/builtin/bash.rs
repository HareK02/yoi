//! Bash Feature projection. The view is instance-owned and shared only by explicit parent
//! routing. No output is indexable; committed result entries directly address immutable files.
//! Root namespace knowledge grants nothing: every publication and read checks the current view.

pub(crate) mod permission_declaration;

use crate::{
    file_operation::{self, Contract},
    wip::{
        WipCallContext, WipMountError, WipMountRegistry, WipOperationError, WipOperationHandler,
        WipOperationOutput, WipProjection, WipProjectionKind, WipPublication, WipSubtreeMount,
        WipSubtreeProvider, contextual_reference, json_to_wip, wip_to_json,
    },
};
use agen::tool::{Tool, ToolError};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use tools::{BashTool, bash_output::BashOutputView};
use wip_protocol::{
    Documentation, FieldDeclaration, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object,
    OperationDeclaration, ParameterDeclaration, ProtocolError, ProtocolErrorCode,
    ReturnDeclaration, TypeExpr, Value,
};
use workdir::{CommandStream, WorkdirSessionRouter};

const ROOT: &str = "/bash-output";

pub(crate) fn mount(
    registry: &mut WipMountRegistry,
    router: Arc<WorkdirSessionRouter>,
    view: BashOutputView,
    enabled: bool,
) -> Result<(), WipMountError> {
    if !enabled {
        return Ok(());
    }
    registry.allocate_namespace("bash", "bash-output")?;
    let provider = Arc::new(Provider {
        service: BashTool::new(router, view.clone()),
        view,
        enabled,
        incarnation: uuid::Uuid::now_v7().to_string(),
    });
    registry.mount(provider.root())?;
    registry.replace_compatibility_tools(ROOT, ["Bash"])?;
    registry.mount_subtree(WipSubtreeMount {
        root: ROOT.into(),
        provider: Arc::new(provider),
    })
}

struct Provider {
    service: BashTool,
    view: BashOutputView,
    enabled: bool,
    incarnation: String,
}
impl Provider {
    fn available(&self) -> bool {
        self.enabled && self.view.is_active()
    }
    fn root(self: &Arc<Self>) -> WipProjection {
        let interface = contextual_reference("yoi.bash/command/v1", ROOT);
        let descriptor = InterfaceDescriptor {
            format: INTERFACE_FORMAT_V1.into(),
            documentation: Some(Documentation {
                summary: "Run Bash with bounded ephemeral output; retained bytes are accessed via result entries, not listed".into(),
                details: None,
            }),
            types: vec![],
            operations: vec![OperationDeclaration {
                name: "run".into(),
                documentation: None,
                parameters: vec![ParameterDeclaration {
                    name: "input".into(),
                    required: true,
                    documentation: Some(Documentation {
                        summary: "Bash input, validated by the existing Tool argument contract".into(),
                        details: Some(BashTool::input_schema().to_string()),
                    }),
                    r#type: TypeExpr::Json,
                }],
                returns: ReturnDeclaration {
                    documentation: None,
                    r#type: TypeExpr::Record {
                        fields: vec![
                            field("summary", TypeExpr::String),
                            field("stdout_preview", TypeExpr::String),
                            field("stderr_preview", TypeExpr::String),
                            field("preview_omitted", TypeExpr::Boolean),
                            field("metadata", TypeExpr::Json),
                            field("stdout_entry", TypeExpr::Entry),
                            field("stderr_entry", TypeExpr::Entry),
                            field("order_entry", TypeExpr::Entry),
                        ],
                    },
                },
            }],
        };
        WipProjection {
            route: ROOT.into(),
            capability: "bash:command".into(),
            kind: WipProjectionKind::Native,
            object: Object {
                name: "bash-output".into(),
                description: Some("Bash command and nonindexable retained output".into()),
                interfaces: vec![interface.clone()],
                r#ref: Some(format!("bash:{}", self.incarnation)),
                validator: Some(self.incarnation.as_bytes().to_vec()),
            },
            interface,
            interface_validator: Some(Sha256::digest(format!("{descriptor:?}")).to_vec()),
            descriptor,
            handler: Arc::new(Run {
                provider: self.clone(),
            }),
        }
    }
    fn parse(&self, path: &str) -> Option<(String, Option<CommandStream>)> {
        let rest = path.strip_prefix("/bash-output/")?;
        let (id, stream) = rest.split_once('/')?;
        if uuid::Uuid::parse_str(id).ok()?.to_string() != id {
            return None;
        }
        let stream = match stream {
            "stdout" => Some(CommandStream::Stdout),
            "stderr" => Some(CommandStream::Stderr),
            "order" => None,
            _ => return None,
        };
        Some((id.into(), stream))
    }
}

#[async_trait]
impl WipSubtreeProvider for Arc<Provider> {
    async fn publication(&self, path: &str) -> Result<Option<WipPublication>, ProtocolError> {
        if !self.available() {
            return Ok(None);
        }
        if path == ROOT {
            let projection = self.root();
            return Ok(Some(WipPublication {
                scope: Some(projection.object.clone()),
                projection,
            }));
        }
        let Some((id, stream)) = self.parse(path) else {
            return Ok(None);
        };
        let Some(metadata) = self.view.metadata(&id) else {
            return Ok(None);
        };
        let contracts = vec![
            Contract::ReadLines,
            Contract::ReadBytes,
            Contract::SearchText,
        ];
        let primary = contracts[0];
        let interface = primary.reference();
        let descriptor = primary.descriptor();
        let validator = Sha256::digest(format!("{}:{id}:{stream:?}", self.incarnation)).to_vec();
        let mut projection = WipProjection {
            route: path.into(),
            capability: "bash:output".into(),
            kind: WipProjectionKind::Native,
            object: Object {
                name: path.rsplit('/').next().unwrap().into(),
                description: Some(format!(
                    "Read-only output component (stdout={} bytes, stderr={} bytes); text is UTF-8 lossy; storage_truncated={}",
                    metadata.stdout_saved, metadata.stderr_saved, metadata.storage_truncated
                )),
                interfaces: vec![interface.clone()],
                r#ref: Some(format!("bash:{}:{id}:{stream:?}", self.incarnation)),
                validator: Some(validator.clone()),
            },
            interface,
            descriptor: descriptor.clone(),
            interface_validator: Some(primary.validator()),
            handler: Arc::new(File {
                provider: self.clone(),
                id,
                stream,
                validator,
            }),
        };
        file_operation::expose(&mut projection, contracts);
        Ok(Some(WipPublication {
            projection,
            scope: None,
        }))
    }
    async fn children(&self, _: &str) -> Result<Vec<String>, ProtocolError> {
        Ok(vec![])
    }
}

struct Run {
    provider: Arc<Provider>,
}
#[async_trait]
impl WipOperationHandler for Run {
    fn is_visible(&self) -> bool {
        self.provider.available()
    }
    fn operation_available(&self, op: &str) -> bool {
        op == "run" && self.provider.available()
    }
    async fn call(
        &self,
        op: &str,
        args: &BTreeMap<String, Value>,
        ctx: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        if op != "run" {
            return Err(fail(
                ProtocolErrorCode::OperationNotFound,
                "unknown Bash operation",
            ));
        }
        let input = args
            .get("input")
            .ok_or_else(|| fail(ProtocolErrorCode::InvalidArguments, "input is required"))?;
        let input =
            wip_to_json(input).map_err(|e| fail(ProtocolErrorCode::InvalidArguments, &e))?;
        if !self.provider.available() {
            return Err(fail(
                ProtocolErrorCode::PermissionDenied,
                "Bash not permitted",
            ));
        }
        let result = self
            .provider
            .service
            .execute_result(&input.to_string(), ctx.execution)
            .await
            .map_err(tool_error)?;
        if result.cancelled {
            return Err(WipOperationError::Cancelled(result.tool_output()));
        }
        if result.metadata.terminal.status == Some(workdir::CommandStatus::TimedOut) {
            return Err(WipOperationError::Interrupted(result.tool_output()));
        }
        // Entry fields are typed by the native Descriptor. No display-string parsing is involved.
        Ok(WipOperationOutput::native(Value::Record(BTreeMap::from([
            ("summary".into(), Value::String(result.summary.clone())),
            (
                "stdout_preview".into(),
                Value::String(result.stdout_preview.clone()),
            ),
            (
                "stderr_preview".into(),
                Value::String(result.stderr_preview.clone()),
            ),
            (
                "preview_omitted".into(),
                Value::Boolean(result.preview_omitted),
            ),
            ("stdout_entry".into(), Value::String(result.stdout_entry())),
            ("stderr_entry".into(), Value::String(result.stderr_entry())),
            ("order_entry".into(), Value::String(result.order_entry())),
            (
                "metadata".into(),
                json_to_wip(&serde_json::to_value(result.metadata).unwrap())
                    .map_err(|e| fail(ProtocolErrorCode::Internal, &e))?,
            ),
        ]))))
    }
    async fn cancel(&self, ctx: &agen::tool::ToolExecutionContext) -> Result<(), ToolError> {
        self.provider.service.cancel_execution(ctx).await
    }
}

struct File {
    provider: Arc<Provider>,
    id: String,
    stream: Option<CommandStream>,
    validator: Vec<u8>,
}
impl File {
    fn order_text(&self) -> Result<String, WipOperationError> {
        use std::fmt::Write;
        let ranges = self
            .provider
            .view
            .order(&self.id)
            .ok_or_else(|| fail(ProtocolErrorCode::NotFound, "output expired or revoked"))?;
        let mut text = String::new();
        for r in ranges {
            writeln!(&mut text, "{:?} {} {}", r.stream, r.start, r.end).unwrap();
        }
        Ok(text)
    }
}
#[async_trait]
impl WipOperationHandler for File {
    fn is_visible(&self) -> bool {
        self.provider.available() && self.provider.view.metadata(&self.id).is_some()
    }
    fn object_validator(&self) -> Option<Vec<u8>> {
        self.is_visible().then(|| self.validator.clone())
    }
    fn operation_available(&self, op: &str) -> bool {
        match op {
            "read_lines" | "read_bytes" | "read" => self.provider.available(),
            "search_text" | "search" => self.provider.available(),
            _ => false,
        }
    }
    async fn call(
        &self,
        op: &str,
        args: &BTreeMap<String, Value>,
        _: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        if !self.operation_available(op) {
            return Err(fail(
                ProtocolErrorCode::PermissionDenied,
                "output operation not permitted",
            ));
        }
        let metadata = self
            .provider
            .view
            .metadata(&self.id)
            .ok_or_else(|| fail(ProtocolErrorCode::NotFound, "output expired or revoked"))?;
        if op == "read_bytes" {
            let (offset, limit) = file_operation::byte_range(args)?;
            let (bytes, total) = if let Some(stream) = self.stream {
                let total = if stream == CommandStream::Stdout {
                    metadata.stdout_saved
                } else {
                    metadata.stderr_saved
                };
                if offset > total as u64 {
                    return Err(fail(
                        ProtocolErrorCode::InvalidArguments,
                        "byte offset exceeds output length",
                    ));
                }
                let bytes = self
                    .provider
                    .view
                    .read(&self.id, stream, offset as usize, limit)
                    .ok_or_else(|| {
                        fail(ProtocolErrorCode::NotFound, "output expired or revoked")
                    })?;
                (bytes, total)
            } else {
                let text = self.order_text()?;
                if offset > text.len() as u64 {
                    return Err(fail(
                        ProtocolErrorCode::InvalidArguments,
                        "byte offset exceeds output length",
                    ));
                }
                (
                    text.as_bytes()[offset as usize..(offset as usize + limit).min(text.len())]
                        .to_vec(),
                    text.len(),
                )
            };
            let eof = offset + bytes.len() as u64 == total as u64;
            return Ok(WipOperationOutput::native_with_validator(
                file_operation::bytes_result(args, &bytes, eof)?,
                self.validator.clone(),
            ));
        }
        let (offset, limit) = file_operation::window(args, op == "search_text")?;
        let value = if let Some(stream) = self.stream {
            let (text, more) = self
                .provider
                .view
                .line_window(&self.id, stream, offset, limit)
                .ok_or_else(|| fail(ProtocolErrorCode::NotFound, "output expired or revoked"))?
                .map_err(|e| fail(ProtocolErrorCode::ResourceLimitExceeded, &e))?;
            file_operation::page_result(op, args, &text, offset, more)?
        } else {
            file_operation::text_result(op, args, &self.order_text()?)?
        };
        Ok(WipOperationOutput::native_with_validator(
            value,
            self.validator.clone(),
        ))
    }
}

fn fail(code: ProtocolErrorCode, message: &str) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code,
        message: message.into(),
    })
}
fn tool_error(error: ToolError) -> WipOperationError {
    match error {
        ToolError::InvalidArgument(_) => fail(ProtocolErrorCode::InvalidArguments, "invalid Bash arguments"),
        ToolError::ExecutionFailure(output) => WipOperationError::ExecutionFailure(output),
        ToolError::Cancelled(output) => WipOperationError::Cancelled(output),
        ToolError::Interrupted(output) => WipOperationError::Interrupted(output),
        _ => WipOperationError::OutcomeUnknown("Bash execution failed; effects or process termination may be unconfirmed; do not automatically restart the command".into()),
    }
}
fn field(name: &str, r#type: TypeExpr) -> FieldDeclaration {
    FieldDeclaration {
        name: name.into(),
        required: true,
        documentation: None,
        r#type,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_bash_handlers_reject_execution_and_output_without_provider_dispatch() {
        let router = Arc::new(WorkdirSessionRouter::new());
        let view = BashOutputView::new();
        let provider = Arc::new(Provider {
            service: BashTool::new(router, view.clone()),
            view,
            enabled: false,
            incarnation: "disabled".into(),
        });
        let context = || WipCallContext {
            result_namespaces: Default::default(),
            execution: Default::default(),
            security_context: "test".into(),
        };
        assert!(provider.publication(ROOT).await.unwrap().is_none());
        let run = Run {
            provider: provider.clone(),
        };
        assert!(!run.is_visible());
        assert!(!run.operation_available("run"));
        let input = json_to_wip(&serde_json::json!({"command":"exit 0", "cwd":"."})).unwrap();
        let result = run
            .call("run", &BTreeMap::from([("input".into(), input)]), context())
            .await;
        assert!(
            matches!(result, Err(WipOperationError::Protocol(error)) if error.code == ProtocolErrorCode::PermissionDenied)
        );
        let file = File {
            provider,
            id: "retained".into(),
            stream: None,
            validator: vec![1],
        };
        for operation in ["read_bytes", "read_lines", "search_text"] {
            assert!(!file.operation_available(operation));
            let result = file.call(operation, &BTreeMap::new(), context()).await;
            assert!(
                matches!(result, Err(WipOperationError::Protocol(error)) if error.code == ProtocolErrorCode::PermissionDenied)
            );
        }
    }
}
