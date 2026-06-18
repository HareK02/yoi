//! Plugin package contributions for model-visible Tool schemas.
//!
//! This module registers *enabled* plugin package tool surface definitions and
//! executes Tool calls through the minimal sandboxed `yoi-plugin-wasm-1` WASM
//! ABI. It deliberately does not grant filesystem, network, environment, hook,
//! service, ingress, or richer host API authority; those remain follow-up
//! boundaries.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use llm_worker::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOrigin, ToolOutput,
};
use manifest::plugin::{
    PluginConfig, PluginDiscoveryLimits, PluginSurface, ResolvedPluginRecord,
    read_resolved_plugin_runtime_module,
};
use serde_json::Value;

use super::{
    FeatureDescriptor, FeatureId, FeatureInstallContext, FeatureInstallError, FeatureModule,
    FeatureRuntimeKind, ToolContribution, ToolDeclaration,
};

/// Build Feature modules for enabled plugin packages when the profile exposes
/// the plugin Tool surface feature.
pub fn plugin_tool_features_if_enabled(
    feature_enabled: bool,
    config: &PluginConfig,
) -> Vec<PluginToolFeature> {
    if !feature_enabled {
        return Vec::new();
    }
    plugin_tool_features(config)
}

/// Build Feature modules for enabled plugin packages that declare Tool surfaces.
pub fn plugin_tool_features(config: &PluginConfig) -> Vec<PluginToolFeature> {
    config
        .resolved
        .iter()
        .filter(|record| record.enabled_surfaces.contains(&PluginSurface::Tool))
        .filter(|record| !record.manifest.tools.is_empty())
        .cloned()
        .map(PluginToolFeature::new)
        .collect()
}

#[derive(Clone, Debug)]
pub struct PluginToolFeature {
    record: ResolvedPluginRecord,
    feature_id: FeatureId,
}

impl PluginToolFeature {
    pub fn new(record: ResolvedPluginRecord) -> Self {
        let feature_id = FeatureId::new(format!("plugin:{}:tool", record.identity))
            .expect("source-qualified plugin identity yields non-empty feature id");
        Self { record, feature_id }
    }

    pub fn origin(&self) -> ToolOrigin {
        ToolOrigin {
            kind: "plugin".into(),
            plugin_id: self.record.manifest.id.clone(),
            plugin_ref: self.record.identity.to_string(),
            source: self.record.identity.source.to_string(),
            digest: self.record.digest.clone(),
            package_version: self.record.version.clone(),
            package_api_version: self.record.manifest.schema_version,
            surface: "tool".into(),
        }
    }
}

impl FeatureModule for PluginToolFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor =
            FeatureDescriptor {
                id: self.feature_id.clone(),
                runtime: FeatureRuntimeKind::ExternalPlugin,
                display_name: self.record.manifest.name.clone(),
                version: self.record.manifest.version.clone(),
                description: self.record.manifest.description.clone().unwrap_or_else(|| {
                    format!("Plugin tool surface from {}", self.record.identity)
                }),
                tools: Vec::new(),
                hooks: Vec::new(),
                background_tasks: Vec::new(),
                provides_services: Vec::new(),
                requires_services: Vec::new(),
                protocol_providers: Vec::new(),
            };
        for tool in &self.record.manifest.tools {
            descriptor = descriptor.with_tool(ToolDeclaration::new(
                tool.name.clone(),
                tool.description.clone(),
            ));
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        validate_declared_tool_names(&self.record)?;
        let origin = self.origin();
        for tool in &self.record.manifest.tools {
            validate_tool_name(&tool.name).map_err(|reason| {
                FeatureInstallError::Install(format!(
                    "plugin `{}` tool `{}` has invalid name: {reason}",
                    self.record.identity, tool.name
                ))
            })?;
            validate_input_schema(&tool.input_schema).map_err(|reason| {
                FeatureInstallError::Install(format!(
                    "plugin `{}` tool `{}` has invalid input_schema: {reason}",
                    self.record.identity, tool.name
                ))
            })?;
            context.tools().register(ToolContribution::new(
                tool.name.clone(),
                plugin_wasm_tool_definition(
                    self.record.clone(),
                    tool.name.clone(),
                    tool.description.clone(),
                    tool.input_schema.clone(),
                    origin.clone(),
                ),
            ))?;
        }
        Ok(())
    }
}

const PLUGIN_WASM_HOST_MODULE: &str = "yoi:tool";
const PLUGIN_WASM_ENTRYPOINT: &str = "yoi_tool_call";
const PLUGIN_WASM_MAX_INPUT_BYTES: usize = 64 * 1024;
const PLUGIN_WASM_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const PLUGIN_WASM_MAX_SUMMARY_BYTES: usize = 1024;
const PLUGIN_WASM_FUEL: u64 = 5_000_000;
const PLUGIN_WASM_TIMEOUT: Duration = Duration::from_secs(1);
const PLUGIN_WASM_MEMORY_BYTES: usize = 2 * 1024 * 1024;
const PLUGIN_WASM_TABLE_ELEMENTS: usize = 256;

fn plugin_wasm_tool_definition(
    record: ResolvedPluginRecord,
    name: String,
    description: String,
    input_schema: Value,
    origin: ToolOrigin,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(name.clone())
                .description(description.clone())
                .input_schema(input_schema.clone())
                .origin(origin.clone()),
            Arc::new(PluginWasmTool {
                record: record.clone(),
                name: name.clone(),
                origin: origin.clone(),
            }) as Arc<dyn Tool>,
        )
    })
}

struct PluginWasmTool {
    record: ResolvedPluginRecord,
    name: String,
    origin: ToolOrigin,
}

#[async_trait]
impl Tool for PluginWasmTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        if input_json.len() > PLUGIN_WASM_MAX_INPUT_BYTES {
            return Err(ToolError::InvalidArgument(format!(
                "plugin tool `{}` input exceeds {} bytes",
                self.name, PLUGIN_WASM_MAX_INPUT_BYTES
            )));
        }
        serde_json::from_str::<Value>(input_json).map_err(|error| {
            ToolError::InvalidArgument(format!(
                "plugin tool `{}` input is not valid JSON: {}",
                self.name,
                bounded_message(error.to_string())
            ))
        })?;

        let record = self.record.clone();
        let name = self.name.clone();
        let plugin_ref = self.origin.plugin_ref.clone();
        let digest = self.origin.digest.clone();
        let input = input_json.as_bytes().to_vec();
        let execution =
            tokio::task::spawn_blocking(move || run_plugin_wasm_tool(record, name, input));
        match tokio::time::timeout(PLUGIN_WASM_TIMEOUT, execution).await {
            Ok(Ok(Ok(output))) => Ok(output),
            Ok(Ok(Err(error))) => Err(ToolError::ExecutionFailed(format!(
                "plugin WASM tool `{}` from `{}` (digest {}) failed closed: {}",
                self.name,
                plugin_ref,
                digest,
                error.bounded_message()
            ))),
            Ok(Err(error)) => Err(ToolError::ExecutionFailed(format!(
                "plugin WASM tool `{}` from `{}` (digest {}) cancelled/failed to join: {}",
                self.name,
                plugin_ref,
                digest,
                bounded_message(error.to_string())
            ))),
            Err(_) => Err(ToolError::ExecutionFailed(format!(
                "plugin WASM tool `{}` from `{}` (digest {}) timed out after {:?}",
                self.name, plugin_ref, digest, PLUGIN_WASM_TIMEOUT
            ))),
        }
    }
}

#[derive(Debug)]
enum PluginWasmError {
    Package(String),
    Module(String),
    Execution(String),
    Output(String),
}

impl PluginWasmError {
    fn bounded_message(&self) -> String {
        match self {
            Self::Package(message) => {
                bounded_message(format!("package/module load error: {message}"))
            }
            Self::Module(message) => bounded_message(format!("WASM module error: {message}")),
            Self::Execution(message) => bounded_message(format!("WASM execution error: {message}")),
            Self::Output(message) => bounded_message(format!("WASM output error: {message}")),
        }
    }
}

#[derive(Debug)]
struct PluginWasmHostState {
    tool_name: Vec<u8>,
    input: Vec<u8>,
    output: Vec<u8>,
    output_error: Option<String>,
    store_limits: wasmi::StoreLimits,
}

fn run_plugin_wasm_tool(
    record: ResolvedPluginRecord,
    tool_name: String,
    input: Vec<u8>,
) -> Result<ToolOutput, PluginWasmError> {
    let limits = PluginDiscoveryLimits::default();
    let module_bytes = read_resolved_plugin_runtime_module(&record, &limits)
        .map_err(|diagnostic| PluginWasmError::Package(diagnostic.message))?;
    if module_bytes.len() > limits.max_file_size_bytes as usize {
        return Err(PluginWasmError::Package(format!(
            "WASM runtime module exceeds {} bytes",
            limits.max_file_size_bytes
        )));
    }

    let mut config = wasmi::Config::default();
    config.consume_fuel(true);
    config.set_max_recursion_depth(64);
    config.set_max_stack_height(8 * 1024 * 1024);
    let engine = wasmi::Engine::new(&config);
    let module = wasmi::Module::new(&engine, &module_bytes[..])
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    validate_wasm_imports(&module)?;

    let store_limits = wasmi::StoreLimitsBuilder::new()
        .memory_size(PLUGIN_WASM_MEMORY_BYTES)
        .table_elements(PLUGIN_WASM_TABLE_ELEMENTS)
        .instances(1)
        .tables(1)
        .memories(1)
        .trap_on_grow_failure(true)
        .build();
    let mut store = wasmi::Store::new(
        &engine,
        PluginWasmHostState {
            tool_name: tool_name.into_bytes(),
            input,
            output: Vec::new(),
            output_error: None,
            store_limits,
        },
    );
    store.limiter(|state| &mut state.store_limits);
    store
        .set_fuel(PLUGIN_WASM_FUEL)
        .map_err(|error| PluginWasmError::Execution(error.to_string()))?;

    let mut linker = wasmi::Linker::<PluginWasmHostState>::new(&engine);
    define_plugin_wasm_host_imports(&mut linker)?;
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|error| PluginWasmError::Execution(error.to_string()))?;
    let entry = instance
        .get_typed_func::<(), ()>(&store, PLUGIN_WASM_ENTRYPOINT)
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    entry
        .call(&mut store, ())
        .map_err(|error| PluginWasmError::Execution(error.to_string()))?;

    if let Some(error) = store.data().output_error.clone() {
        return Err(PluginWasmError::Output(error));
    }
    decode_plugin_wasm_output(&store.data().output)
}

fn validate_wasm_imports(module: &wasmi::Module) -> Result<(), PluginWasmError> {
    for import in module.imports() {
        if import.module() != PLUGIN_WASM_HOST_MODULE {
            return Err(PluginWasmError::Module(format!(
                "unsupported import module `{}`; only `{}` is available",
                import.module(),
                PLUGIN_WASM_HOST_MODULE
            )));
        }
        match import.name() {
            "tool_name_len" | "tool_name_read" | "input_len" | "input_read" | "output_write" => {}
            other => {
                return Err(PluginWasmError::Module(format!(
                    "unsupported host import `{}`; no filesystem, network, environment, or WASI imports are available",
                    other
                )));
            }
        }
    }
    Ok(())
}

fn define_plugin_wasm_host_imports(
    linker: &mut wasmi::Linker<PluginWasmHostState>,
) -> Result<(), PluginWasmError> {
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "tool_name_len",
            |caller: wasmi::Caller<'_, PluginWasmHostState>| -> i32 {
                caller.data().tool_name.len() as i32
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "input_len",
            |caller: wasmi::Caller<'_, PluginWasmHostState>| -> i32 {
                caller.data().input.len() as i32
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "tool_name_read",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                write_host_bytes_to_guest(&mut caller, ptr, len, HostBuffer::ToolName)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "input_read",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                write_host_bytes_to_guest(&mut caller, ptr, len, HostBuffer::Input)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    linker
        .func_wrap(
            PLUGIN_WASM_HOST_MODULE,
            "output_write",
            |mut caller: wasmi::Caller<'_, PluginWasmHostState>, ptr: i32, len: i32| -> i32 {
                read_guest_output(&mut caller, ptr, len)
            },
        )
        .map_err(|error| PluginWasmError::Module(error.to_string()))?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum HostBuffer {
    ToolName,
    Input,
}

fn write_host_bytes_to_guest(
    caller: &mut wasmi::Caller<'_, PluginWasmHostState>,
    ptr: i32,
    len: i32,
    buffer: HostBuffer,
) -> i32 {
    if ptr < 0 || len < 0 {
        return -1;
    }
    let bytes = match buffer {
        HostBuffer::ToolName => caller.data().tool_name.clone(),
        HostBuffer::Input => caller.data().input.clone(),
    };
    if len as usize != bytes.len() {
        return -1;
    }
    let Some(memory) = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
    else {
        return -1;
    };
    match memory.write(caller, ptr as usize, &bytes) {
        Ok(()) => bytes.len() as i32,
        Err(_) => -1,
    }
}

fn read_guest_output(
    caller: &mut wasmi::Caller<'_, PluginWasmHostState>,
    ptr: i32,
    len: i32,
) -> i32 {
    if ptr < 0 || len < 0 {
        caller.data_mut().output_error = Some("guest output pointer/length is invalid".into());
        return -1;
    }
    let len = len as usize;
    if len > PLUGIN_WASM_MAX_OUTPUT_BYTES {
        caller.data_mut().output_error = Some(format!(
            "guest output exceeds {} bytes",
            PLUGIN_WASM_MAX_OUTPUT_BYTES
        ));
        return -1;
    }
    let Some(memory) = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
    else {
        caller.data_mut().output_error = Some("guest did not export linear memory".into());
        return -1;
    };
    let mut output = vec![0; len];
    if memory.read(&*caller, ptr as usize, &mut output).is_err() {
        caller.data_mut().output_error = Some("guest output memory range is invalid".into());
        return -1;
    }
    caller.data_mut().output = output;
    len as i32
}

fn decode_plugin_wasm_output(bytes: &[u8]) -> Result<ToolOutput, PluginWasmError> {
    if bytes.is_empty() {
        return Err(PluginWasmError::Output(
            "guest did not call output_write".into(),
        ));
    }
    if bytes.len() > PLUGIN_WASM_MAX_OUTPUT_BYTES {
        return Err(PluginWasmError::Output(format!(
            "guest output exceeds {} bytes",
            PLUGIN_WASM_MAX_OUTPUT_BYTES
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|error| PluginWasmError::Output(format!("guest output is not UTF-8: {error}")))?;
    let value: Value = serde_json::from_str(text).map_err(|error| {
        PluginWasmError::Output(format!("guest output is not valid JSON: {error}"))
    })?;
    let Value::Object(map) = value else {
        return Err(PluginWasmError::Output(
            "guest output JSON must be an object".into(),
        ));
    };
    for key in map.keys() {
        if key != "summary" && key != "content" {
            return Err(PluginWasmError::Output(format!(
                "guest output contains unsupported key `{key}`"
            )));
        }
    }
    let summary = match map.get("summary") {
        Some(Value::String(summary)) if !summary.is_empty() => summary.clone(),
        Some(Value::String(_)) => {
            return Err(PluginWasmError::Output(
                "guest output summary must not be empty".into(),
            ));
        }
        Some(_) => {
            return Err(PluginWasmError::Output(
                "guest output summary must be a string".into(),
            ));
        }
        None => {
            return Err(PluginWasmError::Output(
                "guest output must include a summary string".into(),
            ));
        }
    };
    if summary.len() > PLUGIN_WASM_MAX_SUMMARY_BYTES {
        return Err(PluginWasmError::Output(format!(
            "guest output summary exceeds {} bytes",
            PLUGIN_WASM_MAX_SUMMARY_BYTES
        )));
    }
    let content = match map.get("content") {
        Some(Value::String(content)) => {
            if content.len() > PLUGIN_WASM_MAX_OUTPUT_BYTES {
                return Err(PluginWasmError::Output(format!(
                    "guest output content exceeds {} bytes",
                    PLUGIN_WASM_MAX_OUTPUT_BYTES
                )));
            }
            Some(content.clone())
        }
        Some(Value::Null) | None => None,
        Some(_) => {
            return Err(PluginWasmError::Output(
                "guest output content must be a string or null".into(),
            ));
        }
    };
    Ok(ToolOutput { summary, content })
}

fn bounded_message(message: impl Into<String>) -> String {
    let message = message.into();
    let mut sanitized = String::with_capacity(message.len().min(512));
    for ch in message.chars() {
        if ch.is_control() && ch != '\n' && ch != '\t' {
            sanitized.push(' ');
        } else {
            sanitized.push(ch);
        }
        if sanitized.len() >= 512 {
            sanitized.truncate(512);
            sanitized.push('…');
            break;
        }
    }
    sanitized
}

fn validate_declared_tool_names(record: &ResolvedPluginRecord) -> Result<(), FeatureInstallError> {
    let mut seen = HashSet::new();
    for tool in &record.manifest.tools {
        if !seen.insert(tool.name.as_str()) {
            return Err(FeatureInstallError::DuplicateToolName {
                tool: tool.name.clone(),
                first_feature: format!("{} (same plugin package)", record.identity),
                duplicate_feature: record.identity.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_tool_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("name must not be empty");
    }
    if name.len() > 128 {
        return Err("name is longer than 128 bytes");
    }
    if name.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("name must not contain whitespace or control characters");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SupportedSchemaType {
    Object,
    Array,
    String,
    Number,
    Integer,
    Boolean,
    Null,
}

impl SupportedSchemaType {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "object" => Some(Self::Object),
            "array" => Some(Self::Array),
            "string" => Some(Self::String),
            "number" => Some(Self::Number),
            "integer" => Some(Self::Integer),
            "boolean" => Some(Self::Boolean),
            "null" => Some(Self::Null),
            _ => None,
        }
    }
}

fn validate_input_schema(schema: &Value) -> Result<(), String> {
    let ty = validate_schema_node(schema, "$", true)?;
    if ty != SupportedSchemaType::Object {
        return Err("root schema type must be `object`".into());
    }
    Ok(())
}

fn validate_schema_node(
    schema: &Value,
    path: &str,
    root: bool,
) -> Result<SupportedSchemaType, String> {
    let Value::Object(map) = schema else {
        return Err(format!("{path}: schema node must be a JSON object"));
    };

    for key in map.keys() {
        if !is_supported_schema_keyword(key) {
            return Err(format!("{path}: unsupported schema keyword `{key}`"));
        }
    }

    let ty = match map.get("type") {
        Some(Value::String(value)) => SupportedSchemaType::parse(value)
            .ok_or_else(|| format!("{path}: unsupported schema type `{value}`"))?,
        Some(_) => return Err(format!("{path}: type must be a string")),
        None if root => return Err("root schema must declare type = `object`".into()),
        None => return Err(format!("{path}: schema node must declare type")),
    };

    if let Some(title) = map.get("title") {
        if !title.is_string() {
            return Err(format!("{path}: title must be a string"));
        }
    }
    if let Some(description) = map.get("description") {
        if !description.is_string() {
            return Err(format!("{path}: description must be a string"));
        }
    }

    let properties = map.get("properties");
    if let Some(properties) = properties {
        if ty != SupportedSchemaType::Object {
            return Err(format!(
                "{path}: properties is only supported for object schemas"
            ));
        }
        let Some(properties) = properties.as_object() else {
            return Err(format!("{path}: properties must be a JSON object"));
        };
        for (name, child_schema) in properties {
            validate_schema_node(child_schema, &format!("{path}.properties.{name}"), false)?;
        }
    }

    if let Some(required) = map.get("required") {
        if ty != SupportedSchemaType::Object {
            return Err(format!(
                "{path}: required is only supported for object schemas"
            ));
        }
        let Some(required) = required.as_array() else {
            return Err(format!("{path}: required must be an array"));
        };
        let mut seen = HashSet::new();
        for entry in required {
            let Some(name) = entry.as_str() else {
                return Err(format!("{path}: required entries must be strings"));
            };
            if !seen.insert(name) {
                return Err(format!("{path}: required entries must be unique"));
            }
            if let Some(properties) = properties.and_then(Value::as_object) {
                if !properties.contains_key(name) {
                    return Err(format!(
                        "{path}: required entry `{name}` is not declared in properties"
                    ));
                }
            }
        }
    }

    if let Some(additional) = map.get("additionalProperties") {
        if ty != SupportedSchemaType::Object {
            return Err(format!(
                "{path}: additionalProperties is only supported for object schemas"
            ));
        }
        match additional {
            Value::Bool(_) => {}
            Value::Object(_) => {
                validate_schema_node(additional, &format!("{path}.additionalProperties"), false)?;
            }
            _ => {
                return Err(format!(
                    "{path}: additionalProperties must be boolean or schema object"
                ));
            }
        }
    }

    if let Some(items) = map.get("items") {
        if ty != SupportedSchemaType::Array {
            return Err(format!("{path}: items is only supported for array schemas"));
        }
        validate_schema_node(items, &format!("{path}.items"), false)?;
    }

    if let Some(enum_values) = map.get("enum") {
        let Some(enum_values) = enum_values.as_array() else {
            return Err(format!("{path}: enum must be an array"));
        };
        if enum_values.is_empty() {
            return Err(format!("{path}: enum must not be empty"));
        }
        for (index, value) in enum_values.iter().enumerate() {
            if enum_values
                .iter()
                .skip(index + 1)
                .any(|other| other == value)
            {
                return Err(format!("{path}: enum entries must be unique"));
            }
        }
    }

    Ok(ty)
}

fn is_supported_schema_keyword(key: &str) -> bool {
    matches!(
        key,
        "type"
            | "title"
            | "description"
            | "properties"
            | "required"
            | "additionalProperties"
            | "items"
            | "enum"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::plugin::{
        PluginDiscoveryOptions, PluginEnablementConfig, PluginPackageManifest,
        PluginRuntimeManifest, SourceQualifiedPluginId, resolve_plugin_config_for_startup,
    };
    use serde_json::json;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn tool(name: &str) -> manifest::plugin::PluginToolManifest {
        manifest::plugin::PluginToolManifest {
            name: name.into(),
            description: format!("{name} tool"),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        }
    }

    fn record(tools: Vec<manifest::plugin::PluginToolManifest>) -> ResolvedPluginRecord {
        record_with_identity("project:example", tools)
    }

    fn record_with_identity(
        identity: &str,
        tools: Vec<manifest::plugin::PluginToolManifest>,
    ) -> ResolvedPluginRecord {
        let parsed_identity = SourceQualifiedPluginId::parse(identity).unwrap();
        ResolvedPluginRecord {
            identity: parsed_identity.clone(),
            source: parsed_identity.source,
            package_path: std::path::PathBuf::from("/tmp/example.zip"),
            package_label: "example.zip".into(),
            digest: "sha256:abc".into(),
            version: "0.1.0".into(),
            manifest: PluginPackageManifest {
                schema_version: 1,
                id: "example".into(),
                name: "Example".into(),
                version: "0.1.0".into(),
                description: None,
                surfaces: vec![PluginSurface::Tool],
                runtime: None,
                hooks: Vec::new(),
                tools,
            },
            enabled_surfaces: vec![PluginSurface::Tool],
            grants: manifest::plugin::PluginGrantConfig::default(),
            config: None,
        }
    }

    fn skipped_count(report: &super::super::FeatureRegistryInstallReport) -> usize {
        report
            .reports
            .iter()
            .map(|feature_report| feature_report.skipped.len())
            .sum()
    }

    fn has_diagnostic(report: &super::super::FeatureRegistryInstallReport, needle: &str) -> bool {
        report.reports.iter().any(|feature_report| {
            feature_report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(needle))
        })
    }

    #[test]
    fn rejects_invalid_root_schema() {
        let schema = json!({"type":"string"});
        assert!(
            validate_input_schema(&schema)
                .unwrap_err()
                .contains("type must be `object`")
        );
    }

    #[test]
    fn rejects_unsupported_schema_keyword() {
        let schema = json!({"type":"object","oneOf":[]});
        assert!(
            validate_input_schema(&schema)
                .unwrap_err()
                .contains("unsupported schema keyword")
        );
    }

    #[test]
    fn rejects_invalid_nested_property_schema_node() {
        let schema = json!({
            "type":"object",
            "properties":{"query":"not-a-schema"},
            "required":["query"],
            "additionalProperties":false
        });
        let error = validate_input_schema(&schema).unwrap_err();
        assert!(error.contains("$.properties.query"));
        assert!(error.contains("schema node must be a JSON object"));
    }

    #[test]
    fn rejects_invalid_recursive_schema_members() {
        let duplicate_required = json!({
            "type":"object",
            "properties":{"query":{"type":"string"}},
            "required":["query", "query"]
        });
        assert!(
            validate_input_schema(&duplicate_required)
                .unwrap_err()
                .contains("required entries must be unique")
        );

        let invalid_items = json!({
            "type":"object",
            "properties":{"values":{"type":"array", "items":"not-a-schema"}}
        });
        assert!(
            validate_input_schema(&invalid_items)
                .unwrap_err()
                .contains("$.properties.values.items")
        );

        let invalid_additional = json!({
            "type":"object",
            "additionalProperties":{"type":"unsupported"}
        });
        assert!(
            validate_input_schema(&invalid_additional)
                .unwrap_err()
                .contains("unsupported schema type")
        );
    }

    #[test]
    fn accepts_object_tool_schema() {
        validate_input_schema(&json!({
            "type":"object",
            "properties":{
                "query":{"type":"string", "description":"Search text"},
                "limit":{"type":"integer", "enum":[1, 5, 10]},
                "tags":{"type":"array", "items":{"type":"string"}}
            },
            "required":["query"],
            "additionalProperties":{"type":"string"}
        }))
        .unwrap();
    }

    #[test]
    fn origin_retains_plugin_metadata() {
        let feature = PluginToolFeature::new(record(Vec::new()));
        let origin = feature.origin();
        assert_eq!(origin.kind, "plugin");
        assert_eq!(origin.plugin_id, "example");
        assert_eq!(origin.plugin_ref, "project:example");
        assert_eq!(origin.source, "project");
        assert_eq!(origin.digest, "sha256:abc");
        assert_eq!(origin.package_version, "0.1.0");
        assert_eq!(origin.package_api_version, 1);
        assert_eq!(origin.surface, "tool");
    }

    #[test]
    fn enabled_plugin_tool_registers_model_visible_schema_and_origin() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("PluginSearch")])))
            .install_into_pending(&mut pending, &mut hooks);

        assert!(
            report
                .reports
                .iter()
                .all(|feature_report| feature_report.diagnostics.is_empty()),
            "{:#?}",
            report.reports
        );
        assert_eq!(report.installed_tool_names(), vec!["PluginSearch"]);
        assert_eq!(pending.len(), 1);
        let (meta, _) = pending[0]();
        assert_eq!(meta.name, "PluginSearch");
        assert_eq!(meta.input_schema["type"], "object");
        let origin = meta.origin.expect("plugin origin metadata");
        assert_eq!(origin.plugin_ref, "project:example");
        assert_eq!(origin.digest, "sha256:abc");
        assert_eq!(origin.source, "project");
        assert_eq!(origin.surface, "tool");
    }

    #[test]
    fn package_without_enabled_tool_surface_registers_no_schema() {
        let mut config = PluginConfig::default();
        let mut disabled = record(vec![tool("PluginSearch")]);
        disabled.enabled_surfaces.clear();
        config.resolved.push(disabled);

        assert!(plugin_tool_features(&config).is_empty());
    }

    #[test]
    fn disabled_profile_feature_registers_no_schema() {
        let mut config = PluginConfig::default();
        config.resolved.push(record(vec![tool("PluginSearch")]));

        assert!(plugin_tool_features_if_enabled(false, &config).is_empty());
        assert_eq!(plugin_tool_features_if_enabled(true, &config).len(), 1);
    }

    #[test]
    fn duplicate_plugin_tool_names_are_rejected_with_diagnostic() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("PluginSearch")])))
            .with_module(PluginToolFeature::new(record_with_identity(
                "project:other",
                vec![tool("PluginSearch")],
            )))
            .install_into_pending(&mut pending, &mut hooks);

        assert_eq!(pending.len(), 1);
        assert_eq!(skipped_count(&report), 1);
        assert!(has_diagnostic(&report, "duplicate tool contribution"));
    }

    #[test]
    fn builtin_tool_name_collision_is_rejected_with_diagnostic() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let mut registered = std::collections::HashMap::new();
        registered.insert("Read".to_string(), FeatureId::builtin("preexisting-tool"));

        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("Read")])))
            .install_into_pending_with_registered(&mut pending, &mut hooks, registered);

        assert!(pending.is_empty());
        assert_eq!(skipped_count(&report), 1);
        assert!(has_diagnostic(&report, "duplicate tool contribution"));
    }

    #[test]
    fn invalid_input_schema_is_rejected_with_diagnostic() {
        let mut invalid = tool("BadSchema");
        invalid.input_schema = json!({"type":"object","$ref":"#/defs/input"});
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();

        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![invalid])))
            .install_into_pending(&mut pending, &mut hooks);

        assert!(pending.is_empty());
        assert!(has_diagnostic(&report, "invalid input_schema"));
    }

    #[test]
    fn nested_invalid_input_schema_does_not_register_plugin_tool() {
        let mut invalid = tool("BadNestedSchema");
        invalid.input_schema = json!({
            "type":"object",
            "properties":{"query":"not-a-schema"},
            "required":["query"],
            "additionalProperties":false
        });
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();

        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![invalid])))
            .install_into_pending(&mut pending, &mut hooks);

        assert!(pending.is_empty());
        assert!(has_diagnostic(&report, "invalid input_schema"));
        assert!(has_diagnostic(&report, "$.properties.query"));
    }

    #[tokio::test]
    async fn registered_plugin_tool_executes_wasm_and_returns_normal_tool_result() {
        let (_dir, record) = resolved_record_with_wasm(input_reaches_guest_module());
        let origin = PluginToolFeature::new(record.clone()).origin();
        let tool = PluginWasmTool {
            record,
            name: "PluginEcho".into(),
            origin,
        };

        let output = tool
            .execute(r#"{"x":1}"#, ToolExecutionContext::default())
            .await
            .unwrap();
        assert_eq!(output.summary, "input reached");
        assert_eq!(output.content.as_deref(), Some("ordinary tool result path"));

        let result = llm_worker::tool::ToolResult::from_output("call-1", output);
        assert_eq!(result.summary, "input reached");
        assert!(
            result
                .content
                .unwrap()
                .contains("ordinary tool result path")
        );
    }

    #[tokio::test]
    async fn malformed_input_json_fails_before_wasm_execution() {
        let (_dir, record) = resolved_record_with_wasm(input_reaches_guest_module());
        let origin = PluginToolFeature::new(record.clone()).origin();
        let tool = PluginWasmTool {
            record,
            name: "PluginEcho".into(),
            origin,
        };

        let error = tool
            .execute("not json", ToolExecutionContext::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("input is not valid JSON"));
    }

    #[tokio::test]
    async fn malformed_output_fails_closed() {
        let (_dir, record) = resolved_record_with_wasm(output_module(b"not json"));
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        assert!(error.bounded_message().contains("not valid JSON"));
    }

    #[tokio::test]
    async fn schema_mismatch_output_fails_closed() {
        let (_dir, record) = resolved_record_with_wasm(output_module(br#"{"summary":1}"#));
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        assert!(error.bounded_message().contains("summary must be a string"));
    }

    #[tokio::test]
    async fn oversize_output_fails_closed() {
        let (_dir, record) = resolved_record_with_wasm(oversize_output_module());
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        assert!(error.bounded_message().contains("exceeds"));
    }

    #[tokio::test]
    async fn nonterminating_execution_fails_closed_with_fuel_boundary() {
        let (_dir, record) = resolved_record_with_wasm(nonterminating_module());
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        let message = error.bounded_message();
        assert!(
            message.contains("Execution")
                || message.contains("fuel")
                || message.contains("execution"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn missing_runtime_module_returns_safe_bounded_tool_error() {
        let record = record_with_missing_package_runtime();
        let origin = PluginToolFeature::new(record.clone()).origin();
        let tool = PluginWasmTool {
            record,
            name: "PluginSearch".into(),
            origin,
        };

        let error = tool
            .execute("{}", ToolExecutionContext::default())
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("failed closed"));
        assert!(message.contains("metadata could not be read"));
        assert!(message.len() < 900);
        assert!(message.contains("project:example"));
    }

    #[tokio::test]
    async fn ambient_wasi_fs_network_env_imports_are_unavailable() {
        let (_dir, record) = resolved_record_with_wasm(wasi_import_module());
        let error =
            run_plugin_wasm_tool(record, "PluginEcho".into(), br#"{}"#.to_vec()).unwrap_err();
        let message = error.bounded_message();
        assert!(message.contains("unsupported import module"), "{message}");
        assert!(message.contains("wasi_snapshot_preview1"), "{message}");
    }

    fn record_with_missing_package_runtime() -> ResolvedPluginRecord {
        let mut record = record(vec![tool("PluginSearch")]);
        record.manifest.runtime = Some(PluginRuntimeManifest {
            kind: "wasm".into(),
            entry: "plugin.wasm".into(),
            abi: Some("yoi-plugin-wasm-1".into()),
        });
        record
    }

    fn resolved_record_with_wasm(wasm: Vec<u8>) -> (TempDir, ResolvedPluginRecord) {
        let dir = TempDir::new().unwrap();
        let package_dir = dir.path().join(".yoi/plugins");
        fs::create_dir_all(&package_dir).unwrap();
        let package_path = package_dir.join("example.yoi-plugin");
        write_plugin_package(&package_path, &wasm);
        let config = PluginConfig {
            enabled: vec![PluginEnablementConfig {
                id: "project:example".parse().unwrap(),
                surfaces: vec![PluginSurface::Tool],
                ..PluginEnablementConfig::default()
            }],
            resolved: Vec::new(),
            diagnostics: Vec::new(),
        };
        let options = PluginDiscoveryOptions::new(dir.path());
        let resolved = resolve_plugin_config_for_startup(&config, &options);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:#?}",
            resolved.diagnostics
        );
        assert_eq!(resolved.resolved.len(), 1);
        (dir, resolved.resolved[0].clone())
    }

    fn write_plugin_package(path: &Path, wasm: &[u8]) {
        let manifest = br#"schema_version = 1
id = "example"
name = "Example"
version = "1.0.0"
description = "Example plugin"
surfaces = ["tool"]

[runtime]
kind = "wasm"
entry = "plugin.wasm"
abi = "yoi-plugin-wasm-1"

[[tools]]
name = "PluginEcho"
description = "Echo plugin tool"
input_schema = { type = "object", additionalProperties = true }
"#;
        write_stored_zip(
            path,
            &[("plugin.toml", manifest.as_slice()), ("plugin.wasm", wasm)],
        );
    }

    fn input_reaches_guest_module() -> Vec<u8> {
        let ok = br#"{"summary":"input reached","content":"ordinary tool result path"}"#;
        let bad = br#"{"summary":"input missing"}"#;
        let wat = format!(
            r#"(module
                (import "yoi:tool" "input_len" (func $input_len (result i32)))
                (import "yoi:tool" "input_read" (func $input_read (param i32 i32) (result i32)))
                (import "yoi:tool" "output_write" (func $output_write (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 0) "{}")
                (data (i32.const 128) "{}")
                (func (export "yoi_tool_call")
                  (local $len i32)
                  (local.set $len (call $input_len))
                  (if (i32.eq (local.get $len) (i32.const 7))
                    (then
                      (drop (call $input_read (i32.const 512) (local.get $len)))
                      (if (i32.eq (i32.load8_u (i32.const 517)) (i32.const 49))
                        (then (drop (call $output_write (i32.const 0) (i32.const {}))))
                        (else (drop (call $output_write (i32.const 128) (i32.const {}))))
                      )
                    )
                    (else (drop (call $output_write (i32.const 128) (i32.const {}))))
                  )
                )
              )"#,
            wat_bytes(ok),
            wat_bytes(bad),
            ok.len(),
            bad.len(),
            bad.len()
        );
        wat::parse_str(wat).unwrap()
    }

    fn output_module(output: &[u8]) -> Vec<u8> {
        let wat = format!(
            r#"(module
                (import "yoi:tool" "output_write" (func $output_write (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 0) "{}")
                (func (export "yoi_tool_call")
                  (drop (call $output_write (i32.const 0) (i32.const {})))
                )
              )"#,
            wat_bytes(output),
            output.len()
        );
        wat::parse_str(wat).unwrap()
    }

    fn oversize_output_module() -> Vec<u8> {
        let wat = format!(
            r#"(module
                (import "yoi:tool" "output_write" (func $output_write (param i32 i32) (result i32)))
                (memory (export "memory") 2)
                (func (export "yoi_tool_call")
                  (drop (call $output_write (i32.const 0) (i32.const {})))
                )
              )"#,
            PLUGIN_WASM_MAX_OUTPUT_BYTES + 1
        );
        wat::parse_str(wat).unwrap()
    }

    fn nonterminating_module() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "yoi_tool_call")
                  (local $remaining i32)
                  (local.set $remaining (i32.const 100000000))
                  (loop $again
                    (local.set $remaining (i32.sub (local.get $remaining) (i32.const 1)))
                    (br_if $again (local.get $remaining))
                  )
                )
              )"#,
        )
        .unwrap()
    }

    fn wasi_import_module() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (import "wasi_snapshot_preview1" "fd_write" (func $fd_write))
                (memory (export "memory") 1)
                (func (export "yoi_tool_call"))
              )"#,
        )
        .unwrap()
    }

    fn wat_bytes(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| format!(r#"\{:02x}"#, byte))
            .collect()
    }

    fn write_stored_zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u32;
            let name_bytes = name.as_bytes();
            let crc = crc32(data);
            write_u32(&mut out, 0x0403_4b50);
            write_u16(&mut out, 20);
            write_u16(&mut out, 0);
            write_u16(&mut out, 0);
            write_u16(&mut out, 0);
            write_u16(&mut out, 0);
            write_u32(&mut out, crc);
            write_u32(&mut out, data.len() as u32);
            write_u32(&mut out, data.len() as u32);
            write_u16(&mut out, name_bytes.len() as u16);
            write_u16(&mut out, 0);
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(data);

            write_u32(&mut central, 0x0201_4b50);
            write_u16(&mut central, 20);
            write_u16(&mut central, 20);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u32(&mut central, crc);
            write_u32(&mut central, data.len() as u32);
            write_u32(&mut central, data.len() as u32);
            write_u16(&mut central, name_bytes.len() as u16);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u32(&mut central, 0);
            write_u32(&mut central, offset);
            central.extend_from_slice(name_bytes);
        }
        let central_offset = out.len() as u32;
        let central_size = central.len() as u32;
        out.extend_from_slice(&central);
        write_u32(&mut out, 0x0605_4b50);
        write_u16(&mut out, 0);
        write_u16(&mut out, 0);
        write_u16(&mut out, files.len() as u16);
        write_u16(&mut out, files.len() as u16);
        write_u32(&mut out, central_size);
        write_u32(&mut out, central_offset);
        write_u16(&mut out, 0);
        fs::write(path, out).unwrap();
    }

    fn write_u16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn write_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
                crc = (crc >> 1) ^ mask;
            }
        }
        !crc
    }
}
