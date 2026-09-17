//! Deterministic OpenAPI 3.1 projection for `#[api(openapi)]` contracts.
//!
//! The projection deliberately has no deployment topology (`servers`) and no
//! generated timestamps. Callers supply an opaque source digest, which is
//! retained as provenance in `info.x-yoi-source-digest`.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::Path,
};

pub use schemars::{self, JsonSchema};
use serde_json::{Map, Value, json};

const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// An explicit assertion that `JsonSchema` describes the type's serialized
/// wire representation.
///
/// OpenAPI-enabled APIs require this trait instead of accepting every
/// `JsonSchema` implementation implicitly. This makes unusual Serde behavior
/// (for example `flatten`, `untagged`, or a custom serializer) fail closed
/// until the type owner supplies and reviews a matching schema implementation.
/// The default component name delegates to [`JsonSchema::schema_name`]; types
/// may override it to provide a stable public wire name.
pub trait OpenApiSchema: JsonSchema {
    fn openapi_schema_name() -> Cow<'static, str> {
        Self::schema_name()
    }
}

macro_rules! impl_openapi_schema {
    ($($ty:ty),* $(,)?) => {
        $(impl OpenApiSchema for $ty {})*
    };
}

impl_openapi_schema!((), bool, String, char, i8, i16, i32, u8, u16, u32, f32, f64);

impl<T: OpenApiSchema> OpenApiSchema for Option<T> {}
impl<T: OpenApiSchema> OpenApiSchema for Vec<T> {}
impl<T: OpenApiSchema> OpenApiSchema for Box<T> {}
impl<T: OpenApiSchema> OpenApiSchema for std::sync::Arc<T> {}
impl<T: OpenApiSchema> OpenApiSchema for BTreeMap<String, T> {}

/// Stable, deployment-independent document identity supplied by an exporter.
#[derive(Debug, Clone, Copy)]
pub struct OpenApiInfo<'a> {
    pub title: &'a str,
    pub version: &'a str,
    /// Opaque digest of the source revision used to build the contract.
    pub source_digest: &'a str,
}

/// An immutable OpenAPI 3.1 document with canonical JSON rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenApiDocument(Value);

impl OpenApiDocument {
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Render pretty JSON with recursively sorted object keys and a final LF.
    pub fn to_json(&self) -> Result<String, OpenApiError> {
        let mut rendered = serde_json::to_string_pretty(&self.0)?;
        rendered.push('\n');
        Ok(rendered)
    }

    /// Export the same canonical bytes returned by [`Self::to_json`].
    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<(), OpenApiError> {
        fs::write(path, self.to_json()?)?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum OpenApiError {
    InvalidContract(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for OpenApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidContract(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "failed to export OpenAPI document: {error}"),
            Self::Json(error) => write!(formatter, "failed to serialize OpenAPI document: {error}"),
        }
    }
}

impl std::error::Error for OpenApiError {}

impl From<std::io::Error> for OpenApiError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for OpenApiError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Builder used by code generated for an `#[api(openapi)]` trait.
#[doc(hidden)]
pub struct OpenApiBuilder {
    info: Value,
    paths: BTreeMap<String, BTreeMap<String, Value>>,
    schemas: BTreeMap<String, Value>,
    operation_ids: BTreeSet<String>,
}

impl OpenApiBuilder {
    pub fn new(info: OpenApiInfo<'_>) -> Result<Self, OpenApiError> {
        for (field, value) in [
            ("title", info.title),
            ("version", info.version),
            ("source digest", info.source_digest),
        ] {
            if value.trim().is_empty() {
                return Err(OpenApiError::InvalidContract(format!(
                    "OpenAPI {field} must not be empty"
                )));
            }
        }

        Ok(Self {
            info: json!({
                "title": info.title,
                "version": info.version,
                "x-yoi-source-digest": info.source_digest,
            }),
            paths: BTreeMap::new(),
            schemas: BTreeMap::new(),
            operation_ids: BTreeSet::new(),
        })
    }

    pub fn operation<'a>(
        &'a mut self,
        method: &'static str,
        path: &'static str,
        operation_id: &'static str,
    ) -> Result<OpenApiOperation<'a>, OpenApiError> {
        let method = method.to_ascii_lowercase();
        if !matches!(
            method.as_str(),
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "trace"
        ) {
            return Err(OpenApiError::InvalidContract(format!(
                "unsupported OpenAPI method `{method}` for `{operation_id}`"
            )));
        }
        if !self.operation_ids.insert(operation_id.to_owned()) {
            return Err(OpenApiError::InvalidContract(format!(
                "duplicate operationId `{operation_id}`"
            )));
        }
        if self
            .paths
            .get(path)
            .is_some_and(|operations| operations.contains_key(&method))
        {
            return Err(OpenApiError::InvalidContract(format!(
                "duplicate operation for {method} {path}"
            )));
        }

        Ok(OpenApiOperation {
            parent: self,
            method,
            path: path.to_owned(),
            operation_id,
            parameters: Vec::new(),
            request_body: None,
            responses: BTreeMap::new(),
        })
    }

    pub fn finish(self) -> OpenApiDocument {
        let paths = self
            .paths
            .into_iter()
            .map(|(path, operations)| {
                let operations = operations.into_iter().collect::<Map<_, _>>();
                (path, Value::Object(operations))
            })
            .collect::<Map<_, _>>();
        let schemas = self.schemas.into_iter().collect::<Map<_, _>>();

        OpenApiDocument(canonicalize(json!({
            "openapi": "3.1.0",
            "info": self.info,
            "paths": paths,
            "components": { "schemas": schemas },
        })))
    }

    fn schema_ref<T: OpenApiSchema>(&mut self) -> Result<Value, OpenApiError> {
        let name = T::openapi_schema_name().into_owned();
        validate_component_name(&name)?;

        let mut schema = serde_json::to_value(schemars::schema_for!(T))?;
        let definitions = schema
            .as_object_mut()
            .and_then(|object| object.remove("$defs"))
            .unwrap_or_else(|| json!({}));
        if let Some(object) = schema.as_object_mut() {
            object.remove("$schema");
            object.remove("title");
        }
        rewrite_definition_refs(&mut schema);
        validate_schema(&schema, &name)?;
        self.insert_schema(name.clone(), canonicalize(schema))?;

        let definitions = definitions.as_object().ok_or_else(|| {
            OpenApiError::InvalidContract(format!(
                "schema `{name}` emitted non-object JSON Schema definitions"
            ))
        })?;
        for (definition_name, definition) in definitions {
            validate_component_name(definition_name)?;
            let mut definition = definition.clone();
            rewrite_definition_refs(&mut definition);
            validate_schema(&definition, definition_name)?;
            self.insert_schema(definition_name.clone(), canonicalize(definition))?;
        }

        Ok(json!({ "$ref": format!("#/components/schemas/{name}") }))
    }

    fn insert_schema(&mut self, name: String, schema: Value) -> Result<(), OpenApiError> {
        match self.schemas.get(&name) {
            Some(existing) if existing != &schema => Err(OpenApiError::InvalidContract(format!(
                "OpenAPI component name collision for `{name}`"
            ))),
            Some(_) => Ok(()),
            None => {
                self.schemas.insert(name, schema);
                Ok(())
            }
        }
    }
}

/// One operation under construction. Generated code adds its complete wire
/// metadata before atomically inserting it into the parent document.
#[doc(hidden)]
pub struct OpenApiOperation<'a> {
    parent: &'a mut OpenApiBuilder,
    method: String,
    path: String,
    operation_id: &'static str,
    parameters: Vec<Value>,
    request_body: Option<Value>,
    responses: BTreeMap<String, Value>,
}

impl OpenApiOperation<'_> {
    pub fn parameter<T: OpenApiSchema>(
        &mut self,
        name: &'static str,
        location: &'static str,
        required: bool,
    ) -> Result<(), OpenApiError> {
        if !matches!(location, "path" | "query" | "header") {
            return Err(OpenApiError::InvalidContract(format!(
                "unsupported parameter location `{location}` for `{}`",
                self.operation_id
            )));
        }
        if location == "path" && !required {
            return Err(OpenApiError::InvalidContract(format!(
                "path parameter `{name}` for `{}` must be required",
                self.operation_id
            )));
        }
        let schema = self.parent.schema_ref::<T>()?;
        self.parameters.push(json!({
            "name": name,
            "in": location,
            "required": required,
            "schema": schema,
            "style": if location == "query" { "form" } else { "simple" },
            "explode": location == "query",
        }));
        Ok(())
    }

    pub fn request_body<T: OpenApiSchema>(
        &mut self,
        content_type: &'static str,
    ) -> Result<(), OpenApiError> {
        if self.request_body.is_some() {
            return Err(OpenApiError::InvalidContract(format!(
                "multiple request bodies for `{}`",
                self.operation_id
            )));
        }
        let schema = self.parent.schema_ref::<T>()?;
        self.request_body = Some(json!({
            "required": true,
            "content": { content_type: { "schema": schema } },
        }));
        Ok(())
    }

    pub fn response<T: OpenApiSchema>(
        &mut self,
        status: u16,
        content_type: &'static str,
        description: &'static str,
    ) -> Result<(), OpenApiError> {
        let schema = self.parent.schema_ref::<T>()?;
        self.insert_response(
            status,
            json!({
                "description": description,
                "content": { content_type: { "schema": schema } },
            }),
        )
    }

    pub fn empty_response(
        &mut self,
        status: u16,
        description: &'static str,
    ) -> Result<(), OpenApiError> {
        self.insert_response(status, json!({ "description": description }))
    }

    fn insert_response(&mut self, status: u16, response: Value) -> Result<(), OpenApiError> {
        if !(100..=599).contains(&status) {
            return Err(OpenApiError::InvalidContract(format!(
                "invalid HTTP response status `{status}` for `{}`",
                self.operation_id
            )));
        }
        let status = status.to_string();
        match self.responses.get(&status) {
            Some(existing) if existing != &response => Err(OpenApiError::InvalidContract(format!(
                "conflicting response status `{status}` for `{}`",
                self.operation_id
            ))),
            Some(_) => Ok(()),
            None => {
                self.responses.insert(status, response);
                Ok(())
            }
        }
    }

    pub fn finish(self) -> Result<(), OpenApiError> {
        if self.responses.is_empty() {
            return Err(OpenApiError::InvalidContract(format!(
                "operation `{}` has no responses",
                self.operation_id
            )));
        }
        let responses = self.responses.into_iter().collect::<Map<_, _>>();
        let mut operation = Map::new();
        operation.insert("operationId".to_owned(), json!(self.operation_id));
        operation.insert("responses".to_owned(), Value::Object(responses));
        if !self.parameters.is_empty() {
            operation.insert("parameters".to_owned(), Value::Array(self.parameters));
        }
        if let Some(request_body) = self.request_body {
            operation.insert("requestBody".to_owned(), request_body);
        }
        self.parent
            .paths
            .entry(self.path)
            .or_default()
            .insert(self.method, Value::Object(operation));
        Ok(())
    }
}

fn validate_component_name(name: &str) -> Result<(), OpenApiError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(OpenApiError::InvalidContract(format!(
            "OpenAPI component name `{name}` must match [A-Za-z0-9._-]+"
        )));
    }
    Ok(())
}

fn rewrite_definition_refs(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref") {
                if let Some(name) = reference.strip_prefix("#/$defs/") {
                    *reference = format!("#/components/schemas/{name}");
                }
            }
            for value in object.values_mut() {
                rewrite_definition_refs(value);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(rewrite_definition_refs),
        _ => {}
    }
}

fn validate_schema(value: &Value, component: &str) -> Result<(), OpenApiError> {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("integer") {
                let safe_format = matches!(
                    object.get("format").and_then(Value::as_str),
                    Some("int8" | "uint8" | "int16" | "uint16" | "int32" | "uint32")
                );
                let minimum = object.get("minimum").and_then(Value::as_i64);
                let maximum = object.get("maximum").and_then(Value::as_u64);
                if !safe_format
                    && (minimum.is_none_or(|value| value < -MAX_SAFE_INTEGER)
                        || maximum.is_none_or(|value| value > MAX_SAFE_INTEGER as u64))
                {
                    return Err(OpenApiError::InvalidContract(format!(
                        "integer schema in `{component}` exceeds the JSON safe-integer range; expose a bounded wire integer"
                    )));
                }
            }
            // A non-null anyOf is commonly emitted for Serde's untagged enum
            // representation. Its branch-selection semantics are not lossless
            // enough for this projection. Nullable schemas remain supported.
            if let Some(Value::Array(branches)) = object.get("anyOf") {
                let null_branches = branches
                    .iter()
                    .filter(|branch| branch.get("type").and_then(Value::as_str) == Some("null"))
                    .count();
                if null_branches != 1 || branches.len() != 2 {
                    return Err(OpenApiError::InvalidContract(format!(
                        "schema `{component}` uses unsupported ambiguous anyOf/untagged semantics"
                    )));
                }
            }
            for value in object.values() {
                validate_schema(value, component)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                validate_schema(value, component)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted = object
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect();
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        scalar => scalar,
    }
}
